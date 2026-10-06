# Iteration 1 (MVP) — first useful data on screen

Goal: a logged-in user opens the app and sees income vs. spending for a period,
as a chart, with the matching transactions and totals. On the way there, the
pipeline flips to its target shape: **Kafka is the source of truth, Postgres is
a rebuildable read model.**

Authoritative inputs: `docs/requirements.md` (decisions), `docs/kafka-migration.md`
(phase 1, dual-write — this spec is phase 2), root `CLAUDE.md`.

---

## 1. Scope

**In scope**

1. Importer stops writing Postgres. It publishes to Kafka only.
2. A new `projector` process consumes the topics and maintains the Postgres read
   model idempotently; replaying from offset 0 reproduces the same tables.
3. Source-agnostic data model: `source` is a first-class column/header, and the
   raw payload is retained per row so nothing is lost to the current schema.
4. Multi-tenancy: `app_user`, `user_account` (many-to-many), sessions. One user
   exists in practice; no roles, no sharing UI.
5. Username/password auth with an HTTP-only cookie session; every GraphQL field
   that touches money is scoped to the caller's accounts.
6. GraphQL: `me`, `login`, `logout`, `accounts`, `transactions` (filter +
   pagination), `cashflowSummary` (income/spending/net buckets) and
   `cashflowGraph` (Sankey-shaped nodes/links, built to grow into categories
   and inter-account transfers without a breaking change).
7. SvelteKit: login page, dashboard (period selector, totals, bar chart, and a
   simple income → account → spending/net Sankey, transaction list).
8. A fully local, bank-free dev stack: Postgres + single-node Redpanda +
   fixture replay, driven from `just`.

**Out of scope** (named so nobody drifts into them)

- Categories, the category tree, the LLM categorizer, held/ambiguous
  transactions, the admin review UI → iteration 2.
- Tags, internal-transfer and investment-saving detection, recurring costs,
  per-transaction user overrides → iteration 3.
- Additional importers (C24, Scalable, PayPal), savings view → iteration 4.
- Goals → `docs/TODO.md`.
- RBAC/ABAC, multi-user sharing UI, invitations, password reset, OAuth/Authelia.
- Balance history charting. Balances are projected (the data is there) but the
  UI does not show them yet.
- Deduplicating one joint account seen through two logins (see §2.8).

**Forward-compatibility constraints iteration 1 must honour**

- The projector owns a fixed set of *derived* columns and nothing else. Later
  enrichment and user overrides live in **separate tables keyed by the
  transaction's identity**, so a replay never clobbers human decisions.
- `transaction.raw_payload` / `account.raw_payload` keep the bank's bytes, so a
  later processor can read fields iteration 1 never mapped without a re-import.
- **Augmentation processors (iteration 2+) operate on the normalized,
  source-agnostic transaction — never on per-source raw JSON.** `raw_payload`
  is an archive for unmapped fields, not an input format: a categorizer parsing
  Comdirect JSON would need rewriting per source and would break outright on
  legacy records (§2.8), which carry no bank payload.
- Nothing in the schema, the topics or the SDL may say "comdirect" outside a
  `source` value or a per-source mapper.

---

## 2. Architecture & data flow

```
Comdirect API ──> import-transactions ──> Kafka (Redpanda) ──> projector ──> Postgres
                      (raw bytes)            4 topics          (mappers)     (read model)
                                                                                  │
 SvelteKit  <── /api/graphql proxy <── actix + async-graphql ──────────────────────┘
```

### 2.1 Topics: reuse, do not rename

The four topics from phase 1 stay exactly as they are (`finreport.account`,
`finreport.account-balance`, `finreport.transaction`,
`finreport.import-watermark`). They carry `prevent_destroy` in
`terraform/kafka`, a rename is a replace, and a replace deletes the log.

They are **retroactively defined as the Comdirect raw topics**. A second source
gets its own topics under `finreport.<source>.<entity>` (e.g.
`finreport.c24.transaction`), with the unprefixed names grandfathered as
`comdirect`. One topic per source keeps the raw-payload rule intact and narrows
mapper selection to `(source, origin)` headers within that source (§2.4).

**Rejected: a common envelope topic** (`{source, type, payload}`) — it would
either re-serialize the bank payload (forbidden, `docs/kafka-migration.md` §2)
or base64-wrap it, making the log unreadable with `rpk`/jq.

### 2.2 Envelope = headers (shared contract)

Every record on every ingest topic carries these headers. This is a contract
frozen by WP0 as `webapp/src/kafka/envelope.rs` and consumed by WP1 (producer)
and WP3 (consumer); neither side may change it unilaterally.

| Header | Required | Value |
|---|---|---|
| `source` | yes | `comdirect` (stable source id, lowercase) |
| `source_account_id` | yes, on all but `account` | the source's own account id (`accountId`) |
| `origin` | yes | `source` (fetched from the provider) or `legacy-backfill` (reconstructed from the old Postgres tables, §2.8) |
| `schema_version` | yes | `1` — bumped only if the *header* contract changes |
| `imported_at` | yes | RFC 3339, when the importer fetched the record |
| `comdirect_account_key` | existing | config key of the login (`0`, `1`, …) |
| `comdirect_account_name` | existing, optional | login label |

`source_account_id` is new and necessary: `finreport.transaction` is keyed by
the transaction `reference` alone, and a Comdirect transaction payload does not
name its account (the endpoint is per-account). The `account` topic's payload
contains it, so the header is redundant there but sent for uniformity.

**Phase-1 records predate these headers** — `producer.rs` only ever set
`comdirect_account_key`, `comdirect_account_name` and `imported_at` — and they
are already in the log. `envelope.rs` therefore defines defaults, applied when
parsing, so the projector never special-cases vintage:

- `source` missing ⇒ `comdirect`; `origin` missing ⇒ `source`;
  `schema_version` missing ⇒ `1`.
- `imported_at` missing ⇒ the record's **Kafka message timestamp** (in the log,
  so replay-stable).
- `source_account_id` missing on a balance or transaction ⇒ **skip + log**.
  Inventing an account would corrupt the read model, and keeping mappers pure
  (§2.4) forbids looking one up. Those records are recovered by the full
  re-walk and by the legacy backfill (§2.8), both of which carry the header.

`envelope.rs` also exposes `is_bank_verbatim(headers) -> bool`
(`origin == source`), so a consumer wanting only the bank's own bytes has one
call to make rather than a convention to remember (§2.8).

Keys stay as they are: `account` and `account-balance` by `account_id`,
`transaction` by `reference`, `import-watermark` by `account_id`.

### 2.3 The projector

New binary `webapp/src/bin/projector.rs` (`[[bin]] name = "projector"`).

- One consumer over the three ingest topics (**not** the watermark topic — that
  is importer-private). It uses `assign()` with explicit partitions and **no
  consumer group**: offsets come from Postgres, so group coordination would add
  a second, conflicting source of truth (and a group would let a second
  instance silently take over half the work while both wrote the same rows).
- **Offsets live in Postgres**: `projection_offset(topic, partition,
  next_offset)`, written in the same DB transaction as the rows it covers and
  used as the `assign()` start position (absent ⇒ `Offset::Beginning`). Rebuild
  = `DELETE FROM projection_offset` + delete the projected rows (§2.8); there
  is no broker-side state to reset.
- Batching: poll up to 500 records or 500 ms, apply them in one transaction,
  commit offsets with them. At-least-once delivery after a crash re-applies at
  most one batch; all writes are upserts on natural keys, so that is a no-op.
- **Conflict rule: first writer owns the account link.** A joint account seen
  through two logins can deliver the same `reference` under two
  `source_account_id`s; `UNIQUE (source, external_id)` makes the second an
  upsert of the first. The `DO UPDATE` set **never includes `account_id`** — the
  transaction stays attached to the account that first claimed it, rather than
  flapping between two rows on every import. Same rule for `account.external_id`
  vs. a shared IBAN. Reconciling the two accounts is iteration 3 work (§2.8).
- Failure policy: a record that fails to **parse/map** is logged at `error`
  with topic/partition/offset and skipped (poison records must not wedge the
  pipeline). A record that fails to **write** aborts the batch; the process
  retries with backoff and, after 5 consecutive failures, exits non-zero so the
  restart policy takes over. Rationale: a mapping bug is permanent, a DB
  outage is not.
- Ordering: all topics are 1 partition, so per-topic order is total. Balances
  may arrive for an account whose `account` record has not been projected yet
  (different topics) → the projector **upserts a stub account row** from
  `source` + `source_account_id`, marked `origin='stub'`, which a later real
  record fills in. No foreign-key failures, no dropped balances.
- **Deterministic ids.** Every projector-owned row's `id` is
  `UUIDv5(FINREPORT_NS, …)`: accounts from `(source, external_id)`,
  transactions from `(source, external_id)`, balances from `(account_id,
  balance_date)`. Random v4 ids would change on every rebuild and break every
  `user_account` link (and any later override table) each time the read model
  is rebuilt. `FINREPORT_NS` is a constant in `envelope.rs`.
- **Replay determinism.** Row timestamps come from the record
  (`imported_at` header → `imported_at` / `first_seen_at` / `observed_at`;
  `updated_at` = the same value), never from `now()`. Two replays of the same
  log produce byte-identical tables, which is what the idempotency test asserts.
- **Rebuild never touches `app_user`, `user_account` or `user_session`.** Those
  are not projections. With deterministic ids, links survive a rebuild
  untouched.

### 2.4 Per-source mapping

```rust
pub struct SourceEvent<'a> {
    pub source: &'a str,
    pub source_account_id: Option<&'a str>,
    pub key: &'a str,
    pub payload: &'a [u8],
    pub imported_at: DateTime<Utc>,
}

pub trait SourceMapper: Send + Sync {
    fn source(&self) -> &'static str;
    fn map_account(&self, e: &SourceEvent) -> Result<AccountRecord, MapError>;
    fn map_balance(&self, e: &SourceEvent) -> Result<BalanceRecord, MapError>;
    fn map_transaction(&self, e: &SourceEvent) -> Result<TransactionRecord, MapError>;
}
```

`AccountRecord` / `BalanceRecord` / `TransactionRecord` are the normalized,
source-free shapes defined in §3, carrying their own `id` (§2.3) and
timestamps so the projector adds nothing non-deterministic. `ComdirectMapper` is the only implementation;
it reuses the structs in `comdirect-rs` for parsing but **must not** push
Comdirect vocabulary into the records (`deptor`, `directDebitMandateId`,
`transactionType.key` → normalized fields + `raw_payload`). Mappers are pure
functions of bytes + headers: no DB, no clock, no network. That is what makes
them unit-testable and replay-deterministic.

Registration is a `HashMap<(&'static str, Origin), Box<dyn SourceMapper>>`
keyed by the `source` and `origin` **headers**, with the topic deciding only
the entity kind (account / balance / transaction). `ComdirectMapper` handles
`(comdirect, source)`, `LegacyMapper` handles `(comdirect, legacy-backfill)`
(§2.8). Unknown pair → log and skip.

### 2.5 Normalization rules (comdirect)

- `amount.value` string → `NUMERIC(20,4)`; `amount.unit` → `currency`
  (default `EUR` when absent).
- `bookingDate` / `valutaDate` → `DATE`. An unparseable booking date fails the
  mapping (skip + error): a transaction without a date is useless to every
  view in this iteration.
- Counterparty: `creditor.holderName` → `remitter.holderName` → `deptor`,
  first non-empty, into `counterparty_name`; `creditor.iban` into
  `counterparty_iban` (nullable). Keeps `pickCounterparty` logic server-side
  instead of in the browser.
- `remittanceInfo` → `description` (empty string → `NULL`).
- `transactionType.key` → `transaction_type` (kept verbatim, source-local, used
  for display only); `transactionType.text` is not stored (it is in
  `raw_payload`).
- `bookingStatus` → `booking_status` (`BOOKED` / `NOTBOOKED`).
- **Balance dates are calendar dates from the record** — Comdirect's balance
  payload date for live records, the legacy row's `date` column for
  reconstructed ones (§2.8) — never `imported_at`. Using the import time would
  file a balance under the day it was *fetched*, silently shifting history
  whenever a re-walk happens.
- `external_id` = Comdirect `reference`.
- Sign convention: keep the source's own sign. Comdirect sends negative for
  outgoing. Income = `amount > 0`, spending = `amount < 0`. Zero-amount
  transactions count as neither and are still listed.

### 2.6 Importer changes

- Delete every Postgres write from `import_transactions.rs` (`account`,
  `account_balance`, `account_transactions` upserts) and the `seaql::init_db`
  call with it. The importer no longer needs `APP_database_url`.
- Publishing stops being best-effort for *failure visibility*: a publish
  failure is now data loss, not a degraded side-channel. Keep the import loop
  alive (the next cycle re-fetches), but **log at `error`, and do not advance
  the account's watermark for that cycle** — advancing it past an unpublished
  transaction would lose it permanently. `publish_best_effort` grows a
  `Result`-returning sibling the transaction loop uses.
- `APP_kafka_brokers` becomes **required** for the importer; unset is a startup
  error, not "publishing disabled". (It stays optional for `webapp`, which
  never publishes.)
- `RecordMeta` gains `source`, `source_account_id`, `schema_version` (§2.2).

### 2.7 Watermarks

Unchanged in mechanism (compacted `finreport.import-watermark`, read at
startup, early-stop pagination with the ordering guard in `comdirect-rs` —
**do not remove that guard**). One change: the watermark is only advanced after
every transaction in the batch has been published successfully (§2.6).

### 2.8 Migration of existing data — legacy backfill, no truncation

Current Postgres rows came from the dual-write path and have no raw payload, so
they cannot be republished as raw bank JSON (the bytes were never kept). They
are still real history that Comdirect's API may no longer return, so they are
**reconstructed into the log, not discarded**:

1. The reshaping migration **renames** `account` / `account_balance` /
   `account_transactions` to `legacy_account` / `legacy_account_balance` /
   `legacy_account_transactions` and creates the new tables (§3) alongside
   them. Nothing is dropped or truncated.
2. `legacy-backfill` (a one-off bin) publishes `legacy_*` rows **into the
   regular topics** — `finreport.account`, `finreport.account-balance`,
   `finreport.transaction` — **under live keys** (`account_id`, `account_id`,
   the transaction's `reference`). No separate legacy topics.
3. **It publishes only keys the topic does not already have.** It first reads
   each target topic to its end, collects the existing key set, and skips those
   keys. This is the critical step: the topics are *compacted*, so a legacy
   record written over a key that already holds raw bytes would become the
   newest record for that key and compaction would eventually **delete the
   phase-1 raw payload**. Key-skipping also makes every re-run idempotent and
   cheap.
4. Tombstone the watermark topic so the next import re-walks the full Comdirect
   history and republishes whatever the bank still returns, raw.
5. The projector builds the read model from offset 0.
6. The `legacy_*` tables are dropped **in a later iteration**, only after
   projected row counts and per-account sums have been verified against them.

**Order matters: backfill (key-skipping) → tombstone watermarks → re-walk**, so
the raw re-walk lands *after* the reconstructed records and compaction
converges the log on raw data. Any other order, or skipping the key check,
trades bank bytes for our own reconstruction.

**Why the regular topics.** Iteration 2+ processors consume the event log; side
topics would mean every one of them learns a second topic set forever, or
silently skips the user's older data.

**Restated raw-payload rule.** A value on the three ingest topics is the bank's
bytes **unless the record carries `origin=legacy-backfill`** — the documented
exception, whose value is our own JSON (the serialized `legacy_*` columns),
because there is no bank payload to forward. Consumers needing verbatim bytes
call `is_bank_verbatim` (§2.2); once the records share a key space, no topic
name could express the distinction anyway.

**Mapper selection is by header, not topic**: `source` → `ComdirectMapper`,
`legacy-backfill` → `LegacyMapper` (§2.4).

**Precedence: a raw record always beats a reconstructed one, in either arrival
order.** The read-model tables carry `origin TEXT NOT NULL` (§3). The projector:

- `origin=source` → upsert unconditionally, setting `origin='source'`.
- `origin=legacy-backfill` → insert when absent; on conflict update **only if
  the stored row is `origin IN ('legacy','stub')`** (`DO UPDATE … WHERE
  transaction.origin <> 'source'`).

That guard is order-independent and so replay-safe: any interleaving converges
on the same state, and a re-run backfill never overwrites a raw-built row.
Contested identity is `(source, external_id)` for transactions and accounts,
`(account_id, balance_date)` for balances. In the **log**, compaction finishes
the job — a raw record published later under a legacy record's key becomes the
survivor; until it runs both may be visible, which is why the projector's guard
is about row state, not arrival order.

`raw_payload` holds the reconstructed JSON for legacy rows. The spelling
difference is deliberate: the **header** is `legacy-backfill` (it names the
producer), the **column** is `legacy` (it names the data's nature), and a stub
account row (§2.3) uses `stub`. Precedence `source` > `legacy` > `stub`.

**Cutover runbook** (one-off, in this order):

1. Stop `finreport-be-importer` and `finreport-be` (nothing writes while the
   schema moves).
2. `make migrate` — renames to `legacy_*`, creates the new tables.
3. Run `legacy-backfill` (key-skipping, §2.8).
4. Tombstone the watermark topic.
5. Start the projector; wait for it to catch up.
6. Start the new importer and `webapp`.
7. Verify projected counts/sums against `legacy_*`, then link accounts to users
   (`user-admin link`) if `APP_projector_default_owner` was not set.

**Joint accounts** (same IBAN under two logins, two `accountId`s) produce two
account rows — the unique IBAN constraint is dropped (§3) precisely so this
logs nothing and corrupts nothing. Deduplication is iteration 3 material.

---

## 3. Postgres schema

All changes via sea-orm migrations in `finreport-rs/migration/src/`, registered
in `lib.rs` in order. After migrating, regenerate entities:
`cd finreport-rs && make migrate && make generate-entities` (never hand-edit
`entity/src/entities`).

Money is `NUMERIC(20,4)` everywhere — never `double`. Ids are `UUID`, generated
in Rust: v4 for user-owned rows, **v5 (deterministic)** for everything the
projector owns (§2.3).

**`m2026…_users`**

```
app_user(id UUID PK, username TEXT NOT NULL UNIQUE, password_hash TEXT NOT NULL,
         display_name TEXT NULL, disabled BOOL NOT NULL DEFAULT false,
         created_at TIMESTAMPTZ NOT NULL)
```
`username` is lowercased by the application (no `citext` dependency); the
unique index is on the stored value.

```
user_account(user_id UUID -> app_user.id ON DELETE CASCADE,
             account_id UUID -> account.id ON DELETE CASCADE,
             created_at TIMESTAMPTZ NOT NULL,
             PRIMARY KEY (user_id, account_id))
```
Index on `account_id` for the reverse lookup. No role column — RBAC is later.

```
user_session(id UUID PK, user_id UUID -> app_user.id ON DELETE CASCADE,
             token_hash BYTEA NOT NULL UNIQUE, created_at TIMESTAMPTZ NOT NULL,
             expires_at TIMESTAMPTZ NOT NULL, last_seen_at TIMESTAMPTZ NOT NULL,
             user_agent TEXT NULL)
```
Index on `expires_at` for pruning. Session lookup joins `app_user` and rejects
`disabled = true`, so disabling a user takes effect on the next request rather
than waiting out a 30-day cookie.

**`m2026…_source_agnostic_read_model`** — renames the three existing tables to
`legacy_*` and creates the new ones beside them. Nothing is truncated (§2.8).

```
account(id UUID PK,
        source TEXT NOT NULL,                  -- 'comdirect'
        external_id TEXT NOT NULL,             -- source's accountId
        display_id TEXT NULL, account_type TEXT NULL,
        iban TEXT NULL, bic TEXT NULL, institute TEXT NULL,
        label TEXT NULL,                       -- was account_name (login label)
        currency TEXT NOT NULL DEFAULT 'EUR',
        raw_payload JSONB NULL,
        origin TEXT NOT NULL,                  -- 'source'|'legacy'|'stub'; §2.8
        first_seen_at TIMESTAMPTZ NOT NULL, updated_at TIMESTAMPTZ NOT NULL,
        UNIQUE (source, external_id))
```
`iban` loses its unique constraint, gains a plain index. `account.id` is the
surrogate key every other table references (the old string `account_id` foreign
keys go away).

```
account_balance(id UUID PK, account_id UUID -> account.id ON DELETE CASCADE,
                balance_date DATE NOT NULL, amount NUMERIC(20,4) NOT NULL,
                currency TEXT NOT NULL, raw_payload JSONB NULL,
                origin TEXT NOT NULL,          -- 'source'|'legacy'; see §2.8
                observed_at TIMESTAMPTZ NOT NULL,
                UNIQUE (account_id, balance_date))
```
Later observation for the same day wins, matching "the balance as of that date".

```
transaction(id UUID PK, account_id UUID -> account.id ON DELETE CASCADE,
            source TEXT NOT NULL, external_id TEXT NOT NULL,
            booking_date DATE NOT NULL, valuta_date DATE NULL,
            booking_status TEXT NOT NULL,
            amount NUMERIC(20,4) NOT NULL, currency TEXT NOT NULL,
            counterparty_name TEXT NULL, counterparty_iban TEXT NULL,
            description TEXT NULL, transaction_type TEXT NULL,
            raw_payload JSONB NOT NULL,
            origin TEXT NOT NULL,              -- 'source'|'legacy'; see §2.8
            imported_at TIMESTAMPTZ NOT NULL, updated_at TIMESTAMPTZ NOT NULL,
            UNIQUE (source, external_id))
```
Renamed from `account_transactions`. Indexes get **new names** —
`idx_transaction_account_booking_date` on `(account_id, booking_date DESC)`,
`idx_transaction_booking_date` on `(booking_date)` — rather than reusing
`idx_account_transactions_account_booking`, which still belongs to the renamed
`legacy_account_transactions`.

`UNIQUE (source, external_id)` is the projector's upsert target and the stable
identity later enrichment/override tables reference. Not
`(account_id, external_id)`: a re-import can re-key the account, not the
source's own id.

```
projection_offset(topic TEXT NOT NULL, partition INT NOT NULL,
                  next_offset BIGINT NOT NULL, updated_at TIMESTAMPTZ NOT NULL,
                  PRIMARY KEY (topic, partition))
```

**Renamed, not dropped**: the three current read-model tables become
`legacy_*` (§2.8) and survive this iteration untouched — the backfill's source,
and the verification baseline before they are dropped later.

**Untouched**: `transactions`, `categories`, `transaction_categories`,
`mandate_categories` — iteration 2 redesigns categories; leave them alone
rather than half-migrating them. The `db_importer` and `categorize` bins are
updated only as far as "still compiles"; `categorize` is rewritten later.

---

## 4. Authentication

**Hashing.** `argon2` crate, Argon2id, default `Params` (m=19456 KiB, t=2,
p=1), per-password random salt via `OsRng`, PHC-string in `password_hash`.
Passwords are `SecretString` (`secrecy`) end-to-end — never a bare `String`,
never logged, never in a `Debug` impl.

**Sessions.** On login: 32 random bytes from `OsRng`, base64url — that string
is the cookie value and is never stored; Postgres keeps `sha256(token)` in
`user_session.token_hash`. TTL 30 days, with a sliding `last_seen_at` refresh
at most once per hour (no write per request). Logout deletes the row; expired
rows are pruned opportunistically on login.

**Cookie.** Name `fr_session`; `HttpOnly`, `SameSite=Lax`, `Path=/`,
`Max-Age` = TTL, `Secure` controlled by `APP_cookie_secure` (default `true`;
the local dev `.env` sets `false` for plain-HTTP localhost). Set via
`ctx.append_http_header("set-cookie", …)` from the `login`/`logout` resolvers,
which the actix integration propagates onto the HTTP response.

**CSRF.** Four cheap layers. Frontend: SvelteKit's `csrf.checkOrigin` stays on,
and the `/api/graphql` proxy (§6) rejects any request whose `Content-Type` is
not `application/json`, forwarding `Content-Type` and `Origin` unchanged so the
backend sees the browser's real values, not the proxy's. Backend:
`SameSite=Lax` blocks cross-site POST cookie attachment; GraphQL requires
`Content-Type: application/json` (not a simple-request type, so a cross-origin
form cannot forge it); `Origin`, when present, must be in
`APP_allowed_origins`. `Cors::default().allow_any_origin()` **must go** —
credentialed CORS and `*` are incompatible — replaced by an explicit
comma-separated allow-list plus `supports_credentials()`.

**Request context.** Actix extracts the cookie, looks the session up
(`token_hash`, `expires_at > now`, user not `disabled`) and injects
`Option<AuthenticatedUser { user_id, username, account_ids }>` into the
async-graphql context via `.data()`; `account_ids` comes from `user_account`
in one query per request.

`graphql/current_user.rs` is rewritten: `current_user(ctx)` returns that user
or a `NotAuthenticated` error; `scoped_account_ids` keeps its signature and
tests, now on `Uuid`. **Every** resolver reading `account`, `account_balance`
or `transaction` goes through it — no unscoped query path. `login` and `me` are
the only unauthenticated fields (`me` returns `null`).

**User and account administration.** New bin `user-admin`
(`webapp/src/bin/user_admin.rs`):

```
user-admin create-user --username <u> [--display-name <n>]   # password from stdin or $FINREPORT_PASSWORD
user-admin set-password --username <u>
user-admin list-users
user-admin list-accounts
user-admin link --username <u> --account <source>:<external-id>|--all
user-admin unlink --username <u> --account <source>:<external-id>
```

Plus optional `APP_projector_default_owner=<username>`: when set, the projector
links every newly created account row to that user in the same transaction;
unset, accounts appear only after an explicit `link`. This is the only place
the projector touches `user_account`, and it never unlinks.

---

## 5. GraphQL API

Shared contract, frozen by WP0 as `finreport-rs/webapp/schema.graphql` (and
mirrored into the frontend) before WP4 or WP5 start. WP0 produces it by
exporting from stub resolvers, so it is the exporter's own canonical output
from the start; `async-graphql` drops comments and imposes its own ordering, so
the drift test **parses both and compares normalized ASTs**, not bytes. `Date`,
`DateTime`, `Decimal` and `UUID` are custom scalar newtypes (not raw
`NaiveDate`/`Uuid`), which is what keeps those type names ours.

```graphql
scalar Date      # "YYYY-MM-DD"
scalar DateTime  # RFC 3339
scalar Decimal   # exact decimal as a string, e.g. "-12.3400"
scalar UUID

type Query {
  me: Me
  accounts: [Account!]!
  transactions(filter: TransactionFilter, page: PageInput): TransactionPage!
  cashflowSummary(filter: TransactionFilter!, granularity: Granularity!): CashflowSummary!
  cashflowGraph(filter: TransactionFilter!, grouping: CashflowGraphInput): CashflowGraph!
}
type Mutation { login(input: LoginInput!): Me!, logout: Boolean! }

input LoginInput { username: String!, password: String! }
type Me { id: UUID!, username: String!, displayName: String }

type Account {
  id: UUID!
  source: String!          # "comdirect"
  externalId: String!
  displayId: String
  accountType: String
  iban: String
  bic: String
  institute: String
  label: String            # login label, display only
  currency: String!
  latestBalance: Balance
}

type Balance { date: Date!, amount: Decimal!, currency: String! }
type Transaction {
  id: UUID!
  accountId: UUID!
  source: String!
  externalId: String!
  bookingDate: Date!
  valutaDate: Date
  bookingStatus: String!
  amount: Decimal!
  currency: String!
  counterpartyName: String
  counterpartyIban: String
  description: String
  transactionType: String
}

input TransactionFilter {
  startDate: Date          # inclusive; null = unbounded
  endDate: Date            # inclusive; null = unbounded
  accountIds: [UUID!]      # null/empty = all of the caller's accounts
  search: String           # case-insensitive substring over counterpartyName + description
  direction: Direction     # null = both
  # Sankey drill-down: clicking a node filters to exactly what it aggregated.
  counterpartyNames: [String!]   # OR-ed exact matches
  hasCounterparty: Boolean       # false selects the "Unknown" node's rows
}

enum Direction { INCOME, SPENDING }
enum Granularity { DAY, WEEK, MONTH }
input PageInput { limit: Int! = 50, offset: Int! = 0 }   # limit clamped to 1..=200
type TransactionPage { items: [Transaction!]!, totalCount: Int!, limit: Int!, offset: Int! }
type CashflowSummary { buckets: [CashflowBucket!]!, total: CashflowTotals!, currency: String! }

type CashflowBucket {
  start: Date!             # inclusive
  end: Date!               # inclusive
  income: Decimal!         # sum of amount > 0, positive
  spending: Decimal!       # sum of |amount| for amount < 0, positive
  net: Decimal!            # income - spending
  transactionCount: Int!
}

type CashflowTotals { income: Decimal!, spending: Decimal!, net: Decimal!, transactionCount: Int! }

# --- Sankey-shaped cash flow -------------------------------------------------
# Deliberately library-neutral (nodes + links, not "sankey"): the same shape
# feeds a chord/alluvial/flow view if LayerChart's Sankey ever disappoints.

input CashflowGraphInput {
  # Left-to-right dimensions. Iteration 1 accepts only the default and errors otherwise.
  dimensions: [CashflowDimension!]! = [INCOME_SOURCE, ACCOUNT, OUTCOME]
  maxNodesPerDimension: Int! = 8   # remainder folded into one "Other" node
}

# CATEGORY (iteration 2) and TAG (iteration 3) are reserved: declared, rejected
# until implemented, so an old server never silently returns a graph it did not build.
enum CashflowDimension { INCOME_SOURCE, ACCOUNT, OUTCOME, CATEGORY, TAG }

type CashflowGraph {
  nodes: [CashflowNode!]!
  links: [CashflowLink!]!
  currency: String!
  dimensions: [CashflowDimension!]!   # echoes what the server applied
  truncated: Boolean!
}

type CashflowNode {
  id: ID!                    # opaque, stable within one response
  label: String!
  kind: CashflowNodeKind!
  depth: Int!                # column index; the server lays out, not the client
  value: Decimal!            # max(inflow, outflow)
  refType: String            # e.g. "account" | "category" — null for synthetic nodes
  refId: String              # id within refType, for drill-down
}

enum CashflowNodeKind { INCOME_SOURCE, ACCOUNT, SPENDING, NET, DEFICIT, OTHER, CATEGORY, TAG }
type CashflowLink { sourceId: ID!, targetId: ID!, value: Decimal! }   # value always positive
```

Semantics and edge cases:

- `transactions` orders by `bookingDate DESC, externalId DESC` — a stable tie-break,
  otherwise paging through a day with many transactions can repeat or skip rows.
- `cashflowSummary` requires a bounded range (`startDate` **and** `endDate`),
  else a validation error — unbounded aggregation has no sensible bucket list.
- Buckets are **dense**: periods with no transactions are returned with zeros,
  so the chart has no gaps. `WEEK` starts Monday (ISO). `MONTH` buckets are
  calendar months, clipped to the requested range at both ends.
- Aggregation runs in Postgres (`date_trunc` + `SUM(…) FILTER (WHERE …)`), not
  in Rust; dense-filling of empty buckets is done in Rust (pure, unit-tested).
- `currency`: iteration 1 assumes one currency per account set and returns the
  first seen (`EUR`); a mixed set still aggregates and logs a warning — no
  conversion, no error. Multi-currency is a later problem.
- A `UUID` in `accountIds` the caller cannot see is an error, not a silent drop
  (matching today's `scoped_account_ids`); a caller with no linked accounts gets
  empty lists and zero totals, not an error.
- Unauthenticated access to anything but `me`/`login` returns a GraphQL error
  with `extensions.code = "UNAUTHENTICATED"`; the frontend keys its redirect
  off that code.
- `login` with bad credentials returns `extensions.code = "INVALID_CREDENTIALS"`
  with the same message and the same latency for unknown-user and wrong-password
  (hash a dummy PHC string when the user does not exist) — no user enumeration.

**`cashflowGraph` semantics and why it extends without breaking**

- Iteration 1 builds exactly `INCOME_SOURCE → ACCOUNT → OUTCOME`: incoming
  transactions grouped by `counterparty_name` (top N + `Other`) flow into their
  account; each account flows out into spending counterparties (top N +
  `Other`) and, when income exceeds spending, a single `NET` node.
- **Flow conservation is per node, not per graph.** Each account must balance
  on its own: an account whose income exceeds its spending gets a `NET` node on
  the right, one whose spending exceeds its income gets a `DEFICIT` node at
  depth 0 feeding it ("from reserves"). **Both can appear in the same graph** —
  one account saving while another overspends is the normal case for a
  household, and a single graph-level net would hide it. Link values are always
  positive; sign lives in node `kind`.
- **Acyclicity is the server's job** (d3-sankey needs a DAG). Free in iteration
  1 by construction; iteration 3's account-to-account transfers can create
  mutual flows, which the server nets into one directed link. The client never
  deduplicates cycles.
- Empty period → `nodes: [], links: []`, not an error. A single transaction
  still yields a valid two-link graph.
- Transactions with no counterparty name land in an `Unknown` node of the same
  kind (never dropped, or the totals would silently stop matching
  `cashflowSummary`). `Other` and `Unknown` nodes drill down through
  `counterpartyNames` / `hasCounterparty` on `TransactionFilter`.
- **Non-breaking growth path:** iteration 2 allows
  `[INCOME_SOURCE, ACCOUNT, CATEGORY]`, iteration 3 adds `TAG` and
  account-to-account links — purely *additive enum values and node kinds*, no
  field or type changes, no new query. What makes that work: clients treat
  unknown `kind` values as `OTHER` and never infer layout from `kind` (that is
  `depth`'s job), and the server echoes the `dimensions` it applied.
  `cashflowSummary` is unaffected; the two queries answer different questions
  and neither subsumes the other.

---

## 6. Frontend

**Same-origin GraphQL proxy.** A cross-origin cookie
(`localhost:5173` → `localhost:8080`) needs `SameSite=None; Secure`, which does
not work over plain-HTTP localhost, so SvelteKit proxies:
`src/routes/api/graphql/+server.ts` forwards the POST body and `cookie` header
to the backend (private `GRAPHQL_URL`, default `http://localhost:8080/graphql`)
and relays `set-cookie` back. `graphqlClient.ts` points at `/api/graphql` with
`credentials: 'include'`. `PUBLIC_GRAPHQL_URL` is retired; `.env.tower` sets
`GRAPHQL_URL`. The `filterSerializedResponseHeaders` note in
`finreport-fe/CLAUDE.md` is updated, not removed.

**Auth handling.** `hooks.server.ts` resolves `me` once per request (cookie
forwarded) into `event.locals.user`; `(app)/+layout.server.ts` exposes it and
redirects to `/login?redirectTo=…` when it is null (`/login` sits outside that
group). Login is a form action → `login` mutation → relay `set-cookie` →
redirect.

**Routes**

- `/login` — username/password form, generic error message, no "username not
  found".
- `/` — dashboard. Period selector (This month / Last month / Last 3 months /
  This year / Custom range) + granularity (day/week/month, defaulting to the
  range: ≤ 31 days → day, ≤ 26 weeks → week, else month). Totals row
  (income, spending, net). Bar chart of the buckets, and below it the
  income → account → spending/net Sankey for the same period. Transaction list
  (paged, 50/page, newest first), filtered by the period and narrowed by
  clicking a bucket or a Sankey node/link (node → that counterparty or account,
  link → both ends).
- `/transactions` — the existing list page, re-pointed at the new SDL.

**Styling: Tailwind CSS 4, and nothing else.** `finreport-fe` already has
Tailwind 4 wired through `@tailwindcss/vite` with `@tailwindcss/forms` and
`@tailwindcss/typography` enabled in `src/app.css`. Iteration 1 adds **no
component library and no second CSS system** — no DaisyUI, Skeleton,
shadcn-svelte, bits-ui, Flowbite, no CSS modules, no Sass. Rules:

- All styling is Tailwind utilities in markup. Svelte `<style>` blocks are not
  used; the two existing ones (`+layout.svelte`'s `.tabs`,
  `transactions/+page.svelte`) are converted, so the codebase ends with one
  styling mechanism, not two.
- Repeated class strings become **Svelte components** (`Button`, `Card`,
  `Table`, `Field` in `src/lib/components/`), not `@apply` — `@apply` recreates
  a second, invisible style layer and is how Tailwind codebases drift back into
  bespoke CSS.
- `src/app.css` is unchanged except for `@theme` tokens when a value is genuinely
  shared (brand colour, the income/spending/net pair). Those tokens feed both UI
  and chart marks, so a chart colour cannot drift from its legend.
- Form controls lean on `@tailwindcss/forms`; period inputs are plain
  `<input type="date">` / `<select>`. No date-picker dependency.
- Layout: mobile-first, one `max-w-6xl mx-auto` column, cards for totals, a grid
  collapsing to one column below `md`. Dark mode is **out of scope** — no `dark:`
  variants, so nobody half-ships a theme.
- Accessibility is part of "done": focus-visible rings kept, amounts carry a
  textual sign (never colour alone), charts get an `aria-label` summarizing the
  period with the transaction list as their table fallback.
- `prettier-plugin-tailwindcss` (installed) orders classes; `npm run lint`
  enforces it.

**Charting: `layerchart`, replacing `chart.js` — decided, not open.**

LayerChart is **the** charting library: every chart, Sankey included. Pin `^2.5`
(the v2 line rebuilt for Svelte 5 runes/snippets); v1 is Svelte 4. No second
charting dependency — a chart LayerChart cannot do is a reason to raise it, not
to quietly install something else. The deciding requirement was Sankey
(categories in iteration 2, account flows in iteration 3): choosing the bar
library without it means replacing it twice, and `chart.js` has no first-class
Sankey.

| | LayerChart 2.x (chosen) | ECharts (+ `svelte-echarts`) |
|---|---|---|
| Sankey | built in, d3-sankey under the hood, node/link props (alignment, node width/padding, link colour) | built in, mature, good labels/tooltips |
| Svelte 5 | native: runes + snippets, components are the API | wrapper component around an imperative `setOption` lifecycle |
| Output | **SVG DOM** → Tailwind utility classes apply to marks directly | `<canvas>` → styling is a JS options object, opaque to Tailwind |
| Theming | reads the same `@theme` tokens as the rest of the UI via classes | colours duplicated into chart options |
| Bundle | tree-shaken components + the `d3-*` modules actually used | smaller than full ECharts when tree-shaken, still the heavier of the two |
| Scope | one library for bars **and** Sankey | one library, but a second styling model |

LayerChart keeps the §6 styling rule honest — SVG marks take Tailwind classes
instead of a parallel canvas-only theme — is Svelte-native, and covers bars and
Sankey with one dependency. `chart.js` is **removed** from `package.json`. The
`cashflowGraph` contract (§5) stays library-neutral nodes/links as insurance,
not as an invitation to re-litigate.

**Iteration 1 Sankey (included — it is cheap).** Below the bar chart, one
Sankey of `income sources → account → spending / net` from `cashflowGraph`:
the same `GROUP BY` plus top-N truncation. Guards: top 8 per side, remainder
folded into `Other`, no categories (iteration 2), no account-to-account links
(iteration 3).

Period math (bucket labels, default granularity, range presets) goes in
`src/lib/period.ts` as pure functions — that is what vitest covers (see
`finreport-fe/CLAUDE.md`: no component-render test setup).

---

## 7. Local dev environment

The whole stack must be demoable with no Comdirect credentials and no network
access to the bank or to `kafka.lab.anydef.de`.

**`docker-compose.local.yml`** gains:

- `finreport-redpanda`: `redpandadata/redpanda`, single node,
  `--mode dev-container --smp 1`, advertising `127.0.0.1:19092` externally and
  `finreport-redpanda:9092` internally, healthcheck `rpk cluster health`.
- `finreport-redpanda-init`: one-shot `rpk topic create` for the four topics with
  the same partitions/cleanup policies as `terraform/kafka/main.tf` (1 partition,
  RF 1; `compact` for account/transaction/watermark, `delete` +
  `retention.ms=-1` for balances). This file and the Terraform module must be
  kept in step — drift means local behaves unlike deployed.

**`just` recipes**

| Recipe | Does |
|---|---|
| `dev-up` | compose up Postgres + Redpanda + topic init, `--wait` |
| `dev-down` | compose down |
| `dev-projector` | `cargo run -p webapp --bin projector` against local PG + `127.0.0.1:19092`; `--until-caught-up` exits at the log end instead of tailing |
| `seed-user` | `user-admin create-user --username dev` (password `dev` from env), idempotent |
| `seed-events` | `cargo run -p webapp --bin fixture-replay -- finreport-rs/webapp/fixtures` |
| `dev-demo` | in order: `dev-up` → `make migrate` → `seed-user` → `seed-events` → `projector --until-caught-up` → link accounts (or rely on `APP_projector_default_owner`) → print next steps |
| `dev-reset` | truncate the read model + `projection_offset`, so the next projector run replays |

`redpanda-console` keeps its existing default but is documented with
`just redpanda-console 127.0.0.1:19092` for the local broker.

**`fixture-replay`** (`webapp/src/bin/fixture_replay.rs`): reads one JSON file
per ingest topic — `accounts.json`, `balances.json`, `transactions.json`, each
an array of raw payloads — and publishes each element verbatim with the §2.2
headers (`source=comdirect`). The watermark topic gets no fixture: it is
importer-private and the projector does not read it. Fixtures are **synthetic
but realistically shaped** (including fields the structs do not model, and a few
headerless phase-1-style records, so both the raw-payload promise and the §2.2
defaults are exercised), span ~6 months across 2 accounts, and contain income
and spending. They double as the integration-test corpus; no real bank data is
ever committed.

---

## 8. Testing

| Area | Level | What |
|---|---|---|
| Mappers | unit | Each `map_*` against fixture payloads: field mapping, missing optional fields, unparseable date → `MapError`, unmodelled fields survive into `raw_payload`, sign convention |
| Bucketing | unit | Dense bucket generation per granularity: empty range, single day, month clipped at both ends, ISO week boundaries, DST-free date-only math |
| Cashflow graph | unit | Node/link building: top-N truncation + `Other` folding, `NET` vs `DEFICIT` branch, flow conservation (sum of links into a node equals sum out), no cycles, unknown counterparty bucketing, empty period |
| Auth | unit | Argon2 hash/verify round-trip, wrong password rejected, token hashing, session expiry boundary |
| Scoping | unit | `scoped_account_ids` (existing tests, ported to `Uuid`) + "no linked accounts" |
| Projector | integration | testcontainers Postgres + Redpanda: replay fixtures → assert row counts/values; replay twice → byte-identical table state (ids and timestamps included); kill mid-batch → restart → no duplicates, no gaps; rebuild → `user_account` links still resolve |
| Precedence | integration | raw-then-legacy and legacy-then-raw for one key both end at `origin='source'` with the raw values; a re-run backfill overwrites nothing; a legacy record fills a `stub` row |
| Backfill | integration | `legacy-backfill` against a topic that already holds a key publishes nothing for it (the raw payload survives); a second run publishes nothing at all |
| GraphQL | integration | Real schema over testcontainers Postgres seeded by the projector: unauthenticated access denied, cross-user account access denied, pagination stability, `cashflowSummary` totals match a hand-computed fixture sum, `cashflowGraph` totals reconcile with `cashflowSummary` for the same filter, unsupported `dimensions` rejected |
| FE logic | vitest | `src/lib/period.ts`, amount/row formatting, `cashflowSummary` → bar dataset shaping, `cashflowGraph` → LayerChart node/link shaping including unknown-`kind` fallback |
| FE smoke | Playwright | login → dashboard renders the bar chart and the Sankey (SVG nodes present) and ≥1 transaction row → logout → redirected to `/login` |

Integration tests use `testcontainers` as a dev-dependency and are gated behind
a `integration` cargo feature so `just test` stays fast and offline; a new
`just test-integration` runs them. The Playwright smoke runs against
`just dev-demo` (seeded fixtures), not against a live bank.

---

## 9. Work packages

Seven packages, built for **maximum parallelism**: presentation and
data-processing work never touch the same files. **WP0 lands every shared
contract first** — schema, SDL, envelope, fixtures, settings keys, module
stubs, Cargo entries — so afterwards each package writes only files it owns.

**Contract freeze (WP0 output).** Three artifacts are the integration surface;
once merged they change only by an explicit, announced amendment to this spec:

1. **Postgres read model** — the migrations in §3 plus the regenerated entities.
2. **GraphQL SDL** — §5, committed as `finreport-rs/webapp/schema.graphql` and
   mirrored to `finreport-fe/src/lib/graphql/schema.graphql`.
3. **Kafka event envelope** — §2.2, as code rather than prose, so producer and
   consumer cannot drift: `webapp/src/kafka/envelope.rs` holds the header
   constants, the **topic name constants**, `RecordMeta`, `SourceEvent`, the
   `FINREPORT_NS` UUID, the parse/build helpers with their defaults, and
   `is_bank_verbatim`. `kafka/mod.rs` is WP0's too (it must declare the module
   and no longer own the topic constants), so WP1 never edits a file WP3 is
   waiting on.

Plus a **fixture corpus** (`finreport-rs/webapp/fixtures/`, §7) and JSON mocks
of each GraphQL operation derived from it
(`finreport-fe/src/lib/graphql/mocks/`) — contract artifacts, not test
scaffolding: the frontend renders real-shaped data without a backend, and the
projector tests assert against the same bytes.

### Shared-file protocol

Only four files are legitimately touched by more than one package, and WP0
pre-populates all of them so later edits are zero-conflict:

- `finreport-rs/webapp/src/lib.rs` — WP0 declares `pub mod auth; pub mod projection;`
  (with empty module files) and the `kafka::envelope` export. Nobody else edits it.
- `finreport-rs/webapp/Cargo.toml` — WP0 adds every dependency this iteration
  needs (`argon2`, `rand_core`, `sha2`, `uuid`, `base64`, `rust_decimal`,
  `testcontainers` dev-dep, the `integration` feature) and all `[[bin]]` entries
  (`projector`, `user-admin`, `fixture-replay`, `legacy-backfill`). Nobody else
  edits it.
- `finreport-rs/migration/src/lib.rs` — WP0 only. No other package adds a migration
  in this iteration; a package that believes it needs one amends the spec instead.
- `finreport-rs/utils/src/settings.rs` — WP0 adds all new keys at once
  (`cookie_secure`, `allowed_origins`, `session_ttl_days`, `projector_default_owner`,
  and making `kafka_brokers` required for the importer entry point).

Everything else below is owned outright by exactly one package. Each package is
developed in its own worktree
(`git worktree add ../finreport-worktrees/<branch> -b <branch>`).

---

### WP0 — Contracts, schema & fixtures (blocking, do first)
**Owns:** `finreport-rs/migration/**`, `finreport-rs/entity/**`,
`finreport-rs/webapp/src/kafka/{envelope.rs,mod.rs}`,
`finreport-rs/webapp/fixtures/**`,
`finreport-rs/webapp/schema.graphql`,
`finreport-fe/src/lib/graphql/{schema.graphql,mocks/**}`,
`finreport-rs/utils/src/settings.rs`, plus the four shared files above.

Migrations from §3 (users, membership, sessions, reshaped read model with its
`origin` columns, the `legacy_*` renames, `projection_offset`) + regenerated
entities; `envelope.rs`; the committed SDL
(exported from stub resolvers so it is the exporter's own output, §5); the fixture
corpus — including **headerless phase-1-style records** (§2.2 defaults) and a
transaction missing `source_account_id`, so the skip path is exercised — and
its derived GraphQL mocks, including a `cashflowGraph` response with
truncation, an `Other` node, an `Unknown` node and **one account in `NET` while
another is in `DEFICIT`**, so the frontend meets the awkward cases first; all settings keys; empty
`auth`/`projection` module files; Cargo deps and bin entries.

*Depends on:* nothing. *Blocks:* everything (briefly — this is a small,
mechanical package, kept deliberately thin so it clears fast).
*Done when:* `make migrate` is clean on an empty and on a populated DB,
`make generate-entities` produces the committed entities, the workspace builds
with empty modules and stub bins, `schema.graphql` parses, and the mock JSON
validates against it.

### WP1 — Importer cutover & fixture replay
**Owns:** `finreport-rs/webapp/src/bin/{import_transactions.rs,fixture_replay.rs,legacy_backfill.rs}`,
`finreport-rs/webapp/src/kafka/{producer.rs,events.rs,watermark.rs}` (not
`mod.rs` or `envelope.rs` — WP0's),
`finreport-rs/comdirect-rs/**`, `docs/kafka-migration.md`.

Remove all Postgres access from the importer (the crate keeps `sea-orm` for
the other bins — see the acceptance criterion); emit the §2.2 headers via `envelope.rs`; require `APP_kafka_brokers`;
publish failure → `error` + watermark not advanced; implement `fixture-replay`
against the WP0 corpus; implement the one-off `legacy-backfill` bin (§2.8:
reads `legacy_*`, publishes into the regular topics under live keys with
`origin=legacy-backfill`); rewrite `docs/kafka-migration.md` for phase 2: Postgres is no longer the source
of truth, and the topics are no longer purely bank bytes — point its
raw-payload rule at `is_bank_verbatim` and the `origin` header.
*Depends on:* WP0. *Parallel with:* WP2–WP6 (no file overlap).
*Done when:* the importer makes no database calls and starts with no
`APP_database_url` set (the crate still depends on `sea-orm` — `webapp` is one
crate — so "no DB access" is the testable criterion), a local run publishes
records carrying every required header, `fixture-replay` fills all four topics,
`rpk topic consume` shows byte-unmodified payloads, and `legacy-backfill`
re-runs idempotently (same keys, same values) against a populated `legacy_*`
set.

### WP2 — Auth core & admin CLI
**Owns:** `finreport-rs/webapp/src/auth/**`,
`finreport-rs/webapp/src/bin/user_admin.rs`.

Argon2id hash/verify, token generation + SHA-256 storage, session
create/verify/revoke/prune, `AuthenticatedUser` loading (user + linked
`account_ids`) as plain library functions — **no actix, no async-graphql** here,
so WP4 can wire them without waiting. `user-admin` subcommands per §4.
*Depends on:* WP0. *Parallel with:* WP1, WP3, WP5, WP6. *Blocks:* WP4's wiring
(API shape is fixed here in the spec, so WP4 can code against it immediately).
*Done when:* unit tests cover hash/verify, wrong-password rejection, token
hashing and the expiry boundary, and `user-admin create-user`/`link` work
against a local DB.

### WP3 — Projector (data processing)
**Owns:** `finreport-rs/webapp/src/projection/**`
(`mod.rs`, `mapper.rs`, `comdirect.rs`, `records.rs`, `offsets.rs`, `upsert.rs`),
`finreport-rs/webapp/src/bin/projector.rs`.

`SourceMapper` trait + `ComdirectMapper` + `LegacyMapper` and the
raw-beats-legacy upsert guard (§2.8), batch consume → map → upsert → offset
commit in one transaction, stub accounts, poison-record skip, retry/exit
policy, optional default-owner linking.
*Depends on:* WP0 (entities + envelope). *Parallel with:* WP1, WP2, WP4, WP5.
Does **not** depend on WP1: it consumes fixture-replayed topics.
*Done when:* replaying the fixture corpus fills the read model, a second replay
changes no rows, reset + replay reproduces identical state, a mid-batch kill
leaves no duplicates and no gaps, and a legacy record and a raw record for the
same identity converge on the raw one **in both arrival orders**.

### WP4 — GraphQL API & HTTP surface
**Owns:** `finreport-rs/webapp/src/graphql/**`, `finreport-rs/webapp/src/main.rs`,
`finreport-rs/webapp/src/bin/graphql_schema_exporter.rs`.

Cookie extraction + auth context injection (calling WP2's library),
`login`/`logout`/`me`, rewritten `current_user`, `accounts`/`transactions`/
`cashflowSummary`/`cashflowGraph`, dense bucketing, Sankey node/link building
(top-N + `Other`, per-account `NET`/`DEFICIT`, conservation),
`Decimal`/`Date`/`DateTime`/`UUID` scalar newtypes, CORS allow-list +
credentials + `Origin` check. **Also deletes
`fs::Files::new("/assets", ".").show_files_listing()` from `main.rs`** — it
serves the process's working directory, listing included, which is an
unauthenticated file-read hole next to a login system. The backend binds
`0.0.0.0:8080` and is LAN-reachable, so this is not theoretical.
*Depends on:* WP0 (entities, SDL), WP2 (auth functions — stub them locally if
WP2 has not merged; the signatures are fixed in §4).
*Done when:* the SDL drift test passes (exporter output vs. committed
`schema.graphql`, compared as normalized ASTs, §5), unauthenticated and
cross-user access are denied by test, `cashflowSummary` matches a hand-computed fixture sum,
`cashflowGraph` conserves flow and reconciles with it, and a real HTTP test
round-trips the login cookie.

### WP5 — Frontend (presentation)
**Owns:** `finreport-fe/**` except the WP0-owned `src/lib/graphql/schema.graphql`
and `src/lib/graphql/mocks/**` (read-only to this package).

`/api/graphql` proxy route, `hooks.server.ts` + layout auth guard, `/login`,
dashboard (period selector, totals, LayerChart bar chart + Sankey, paged
transaction list), `layerchart@^2.5` added and `chart.js` removed from
`package.json` (LayerChart is the only charting dependency),
`src/lib/period.ts`, shared Tailwind-styled primitives in
`src/lib/components/`, conversion of the two remaining Svelte `<style>` blocks
to utilities, updated `/transactions`, updated `finreport-fe/CLAUDE.md`.
*Styling is Tailwind 4 only* (§6) — this package adds no component or CSS
framework dependency; a PR that adds one is rejected on sight.
*Depends on:* WP0's SDL + mocks only. **Never blocked on a running backend**:
development and vitest run against the mock responses; a `PUBLIC_USE_MOCKS=1`
switch in the proxy route returns them without a network call.
*Parallel with:* every backend package, by construction — it shares no file
with any of them.
*Done when:* `npm run check` and `npm run lint` pass, vitest covers the period
and chart-shaping helpers, the UI renders fully from mocks, no `<style>` block
or non-Tailwind styling dependency remains, and the Playwright smoke passes
against `just dev-demo`.

### WP6 — Local dev stack & test harness
**Owns:** `docker-compose.local.yml`, `docker-compose.yml`, `justfile`,
`finreport-rs/webapp/tests/support/**` (shared testcontainers harness — WP3 and
WP4 own their own test files under `webapp/tests/`), `.gitea/**`,
root `CLAUDE.md`,
`terraform/kafka/main.tf` comments kept in step with the local topic init.

Redpanda + topic-init services (the same four topics as today — the legacy
backfill adds none, so `terraform/kafka` is unchanged); the
`dev-up`/`dev-down`/`dev-projector`/
`seed-user`/`seed-events`/`dev-demo`/`dev-reset` recipes; the testcontainers
harness behind the `integration` feature + `just test-integration`; deployed
compose updates (importer loses `APP_database_url`, gains required
`APP_kafka_brokers`; new `finreport-be-projector` service).
*Depends on:* WP0 (bin names/flags are fixed there). *Parallel with:* WP1–WP5
— it owns only orchestration files.
*Done when:* a clean checkout reaches a logged-in dashboard with seeded data
using only documented commands, with no bank credentials and no access to the
central broker.

---

### Parallelism summary

```
WP0  ████ (short, blocking)
     └─> WP1 importer        ─┐
         WP2 auth core       ─┤
         WP3 projector       ─┤  all concurrent, disjoint file sets
         WP4 graphql api *   ─┤  (* soft dep on WP2's signatures)
         WP5 frontend        ─┤
         WP6 dev stack       ─┘
```

Presentation (WP5) and data processing (WP3) share only the SDL and fixtures,
both frozen in WP0. Remaining integration risk: WP4 ↔ WP2 (signatures, §4) and
WP4 ↔ WP5 (the SDL, §5, enforced by the drift test).

---

## 10. Assumptions & open questions

**Assumptions made (decided here, not escalated)**

1. Current Postgres history is **not** expendable: it is renamed to `legacy_*`
   and republished into the regular topics under live keys as reconstructed
   (non-raw) records marked `origin=legacy-backfill`, then re-projected, with
   raw records winning any contested identity. The `legacy_*` tables are
   dropped only after verification, in a later iteration (§2.8).
2. Single currency (EUR) across the board; mixed-currency account sets are
   summed without conversion and logged.
3. Offset storage in Postgres rather than Kafka consumer-group offsets, for
   transactional commit with the projected rows.
4. One projector process, single instance. All topics are 1 partition, so there
   is nothing to scale out, and two instances would fight over
   `projection_offset`. Not enforced with a lock in this iteration.
5. Offset-based pagination, not cursors. A household's transaction list does not
   reach the depth where offset paging hurts.
6. `layerchart` (`^2.5`, Svelte 5) is the single charting library for all
   charts including Sankey — a user decision, not an open trade-off.
   `chart.js` is removed (§6).
7. Tailwind 4 utilities only — no component library, no `@apply`, no dark
   mode in this iteration (§6).
8. Session TTL 30 days, sliding; no "remember me" distinction.

**Noted for iteration 2 (not used in iteration 1)**

- The categorizer's LLM provider will be **Anthropic**, not the OpenAI path the
  current `categorizer` crate uses via `rig` (`rig` ships an Anthropic provider).
  The key flows `TF_VAR_anthropic_api_key` (a single `op://` reference in
  `.env.tpl`, per the `.env.tpl` rule in `CLAUDE.md`) → `APP_anthropic_api_key`,
  loaded through `utils::settings` as a `SecretString`. Iteration 1 neither
  reads nor requires it.

**Open questions (do not block iteration 1)**

- Does Comdirect actually return transactions newest-first? Unverified; the
  runtime guard in `comdirect-rs` is the only protection and must not be removed.
- Joint-account deduplication across two logins (same IBAN, two `accountId`s) —
  deferred to iteration 3, where internal-transfer detection needs account
  identity anyway.
- Whether the broker's retention/compaction settings make a from-zero replay
  reliable indefinitely now that the read model depends on it. Compaction on
  `reference` keeps one record per transaction, which suffices today; revisit
  if a topic ever gets a retention limit.
- Whether `account_balance` should keep full observation history rather than one
  row per account-day once a balance chart exists (iteration 4).
- How LayerChart's Sankey copes with iteration 3's account-to-account flows
  (node counts, label legibility). The library choice is settled; this is
  answered with real data in iteration 3.
- Whether top-N 8 per dimension is the right default; it is a query argument,
  so changing it costs nothing.
