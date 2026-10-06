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
3. Source-agnostic data model: `source` is a first-class column/header; the raw
   payload is retained per row so nothing is lost to the current schema.
4. Multi-tenancy: `app_user`, `user_account` (many-to-many), sessions. One user
   exists in practice; no roles, no sharing UI.
5. Username/password auth with an HTTP-only cookie session; every GraphQL field
   that touches money is scoped to the caller's accounts.
6. GraphQL: `me`, `login`, `logout`, `accounts`, `transactions` (filter +
   pagination), `cashflowSummary` (income/spending/net buckets).
7. SvelteKit: login page, dashboard (period selector, chart, totals,
   transaction list).
8. A fully local, bank-free dev stack: Postgres + single-node Redpanda +
   fixture replay, driven from `just`.

**Out of scope** (named so nobody drifts into them)

- Categories, the category tree, the LLM categorizer, held/ambiguous
  transactions, the admin review UI → iteration 2.
- Tags, internal-transfer detection, investment-saving detection, recurring
  costs, per-transaction user overrides → iteration 3.
- Additional importers (C24, Scalable, PayPal), savings view → iteration 4.
- Goals → `docs/TODO.md`.
- RBAC/ABAC, multi-user sharing UI, invitations, password reset, OAuth/Authelia.
- Balance history charting. Balances are projected (the data is there) but the
  UI does not show them yet.
- Deduplicating one joint account seen through two logins (see §2.8).

**Forward-compatibility constraints iteration 1 must honour**

- The projector owns a fixed set of *derived* columns and nothing else. Later
  enrichment (category, tags, flags) and user overrides live in **separate
  tables keyed by the transaction's identity**, so a replay never clobbers
  human decisions. No enrichment column is ever added to a projector-owned
  table.
- `transaction.raw_payload` / `account.raw_payload` keep the bank's bytes, so a
  later processor can read fields iteration 1 never mapped without a re-import.
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
`comdirect`. One topic per source keeps the raw-payload rule intact and lets
the projector pick a mapper from the topic it polled.

**Rejected: a common envelope topic** (`{source, type, payload}`). It would
either re-serialize the bank payload — the exact thing
`docs/kafka-migration.md` §2 forbids — or base64-wrap it, making the log
unreadable with `rpk`/jq. Source and metadata belong in headers.

### 2.2 Envelope = headers (shared contract)

Every record on every ingest topic carries these headers. This is a contract
between WP1 and WP3; neither side may change it unilaterally.

| Header | Required | Value |
|---|---|---|
| `source` | yes | `comdirect` (stable source id, lowercase) |
| `source_account_id` | yes, on all but `account` | the source's own account id (`accountId`) |
| `schema_version` | yes | `1` — bumped only if the *header* contract changes |
| `imported_at` | yes | RFC 3339, when the importer fetched the record |
| `comdirect_account_key` | existing | config key of the login (`0`, `1`, …) |
| `comdirect_account_name` | existing, optional | login label |

`source_account_id` is new and necessary: `finreport.transaction` is keyed by
the transaction `reference` alone, and a Comdirect transaction payload does not
name its account (the endpoint is per-account). Without the header the
projector cannot attach a transaction to an account. The `account` topic is
keyed by `account_id` and its payload contains it, so the header is redundant
there but still sent for uniformity.

Keys stay as they are: `account` and `account-balance` by `account_id`,
`transaction` by `reference`, `import-watermark` by `account_id`.

### 2.3 The projector

New binary `webapp/src/bin/projector.rs` (`[[bin]] name = "projector"`).

- One consumer, consumer group `finreport-projector-v1`, subscribed to the
  three ingest topics (**not** the watermark topic — that is importer-private).
- `enable.auto.commit = false`. **Offsets live in Postgres**, not in Kafka:
  table `projection_offset(topic, partition, next_offset)`, written in the same
  DB transaction as the rows it covers, and `seek()`-ed on startup. This makes
  the projection effectively-once even for non-idempotent work later, and makes
  "rebuild from scratch" a `DELETE FROM projection_offset` plus a truncate
  rather than a broker-side group reset.
- Batching: poll up to 500 records or 500 ms, apply them in one transaction,
  commit offsets with them. At-least-once delivery after a crash re-applies at
  most one batch; all writes are upserts on natural keys, so that is a no-op.
- Failure policy: a record that fails to **parse/map** is logged at `error`
  with topic/partition/offset and skipped (poison records must not wedge the
  pipeline). A record that fails to **write** aborts the batch; the process
  retries with backoff and, after 5 consecutive failures, exits non-zero so the
  restart policy takes over. Rationale: a mapping bug is permanent, a DB
  outage is not.
- Ordering: all topics are 1 partition, so per-topic order is total. Balances
  may arrive for an account whose `account` record has not been projected yet
  (different topics) → the projector **upserts a stub account row** from
  `source` + `source_account_id` and lets the later `account` record fill it in.
  No foreign-key failures, no dropped balances.

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
source-free shapes defined in §3. `ComdirectMapper` is the only implementation;
it reuses the structs in `comdirect-rs` for parsing but **must not** push
Comdirect vocabulary into the records (`deptor`, `directDebitMandateId`,
`transactionType.key` → normalized fields + `raw_payload`). Mappers are pure
functions of bytes + headers: no DB, no clock, no network. That is what makes
them unit-testable and replay-deterministic.

Registration is a `HashMap<&'static str, Box<dyn SourceMapper>>` keyed by
`source`, with the topic → entity-kind decided by which topic the record came
from. Unknown `source` → log and skip.

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

### 2.8 Migration of existing data

Current Postgres rows came from the dual-write path and have no raw payload, so
they cannot be back-converted into the new shape, and they cannot be
republished to Kafka as raw bank JSON (the bytes were never kept). Therefore:

1. Migrations reshape `account` / `account_balance` / `account_transactions`
   (§3) and **truncate them**. Postgres is a read model now; losing it is a
   rebuild, not a loss.
2. Before the first projector run, tombstone the watermark topic (one
   null-valued record per `account_id`, see `docs/kafka-migration.md` §5) so
   the next import walks the full Comdirect history and republishes it raw.
3. The projector then builds the read model from offset 0.

History older than what Comdirect's transactions endpoint still returns is not
recoverable. That is accepted: it is a personal-finance MVP, and keeping the
old Comdirect-shaped tables around to preserve it would permanently fork the
schema.

**Joint accounts** (same IBAN under two logins, two `accountId`s) still produce
two account rows — the unique IBAN constraint is dropped (§3) precisely so this
logs nothing and corrupts nothing. Deduplication is iteration 3 material
(internal-transfer detection needs the same machinery).

---

## 3. Postgres schema

All changes via sea-orm migrations in `finreport-rs/migration/src/`, registered
in `lib.rs` in order. After migrating, regenerate entities:
`cd finreport-rs && make migrate && make generate-entities` (never hand-edit
`entity/src/entities`).

Money is `NUMERIC(20,4)` everywhere — never `double`. Ids on new tables are
`UUID` (`uuid_generate_v4` is avoided; generate in Rust).

**`m2026…_users`**

```
app_user(id UUID PK, username TEXT NOT NULL UNIQUE, password_hash TEXT NOT NULL,
         display_name TEXT NULL, disabled BOOL NOT NULL DEFAULT false,
         created_at TIMESTAMPTZ NOT NULL)
```
`username` is stored lowercased by the application (no `citext` extension
dependency); the unique index is on the stored value.

```
user_account(user_id UUID -> app_user.id ON DELETE CASCADE,
             account_id UUID -> account.id ON DELETE CASCADE,
             created_at TIMESTAMPTZ NOT NULL,
             PRIMARY KEY (user_id, account_id))
```
Index on `account_id` for the reverse lookup. No role column — RBAC is later,
and an unused column would be guessed at wrongly.

```
user_session(id UUID PK, user_id UUID -> app_user.id ON DELETE CASCADE,
             token_hash BYTEA NOT NULL UNIQUE, created_at TIMESTAMPTZ NOT NULL,
             expires_at TIMESTAMPTZ NOT NULL, last_seen_at TIMESTAMPTZ NOT NULL,
             user_agent TEXT NULL)
```
Index on `expires_at` for pruning.

**`m2026…_source_agnostic_read_model`** — reshapes the three existing tables and
truncates them (§2.8).

```
account(id UUID PK,
        source TEXT NOT NULL,                  -- 'comdirect'
        external_id TEXT NOT NULL,             -- source's accountId
        display_id TEXT NULL, account_type TEXT NULL,
        iban TEXT NULL, bic TEXT NULL, institute TEXT NULL,
        label TEXT NULL,                       -- was account_name (login label)
        currency TEXT NOT NULL DEFAULT 'EUR',
        raw_payload JSONB NULL,
        first_seen_at TIMESTAMPTZ NOT NULL, updated_at TIMESTAMPTZ NOT NULL,
        UNIQUE (source, external_id))
```
`iban` loses its unique constraint, gains a plain index. `account.id` becomes
the surrogate key every other table references (the old string `account_id`
foreign keys go away).

```
account_balance(id UUID PK, account_id UUID -> account.id ON DELETE CASCADE,
                balance_date DATE NOT NULL, amount NUMERIC(20,4) NOT NULL,
                currency TEXT NOT NULL, raw_payload JSONB NULL,
                observed_at TIMESTAMPTZ NOT NULL,
                UNIQUE (account_id, balance_date))
```
Later observation for the same day wins (upsert `DO UPDATE`), matching "the
balance as of that date".

```
transaction(id UUID PK, account_id UUID -> account.id ON DELETE CASCADE,
            source TEXT NOT NULL, external_id TEXT NOT NULL,
            booking_date DATE NOT NULL, valuta_date DATE NULL,
            booking_status TEXT NOT NULL,
            amount NUMERIC(20,4) NOT NULL, currency TEXT NOT NULL,
            counterparty_name TEXT NULL, counterparty_iban TEXT NULL,
            description TEXT NULL, transaction_type TEXT NULL,
            raw_payload JSONB NOT NULL,
            imported_at TIMESTAMPTZ NOT NULL, updated_at TIMESTAMPTZ NOT NULL,
            UNIQUE (source, external_id))
```
Renamed from `account_transactions` (the old name describes a join table it
never was). Indexes: `(account_id, booking_date DESC)` — replaces
`m20260718_000001_idx_account_transactions_account_booking` — and
`(booking_date)` for cross-account aggregation.

`UNIQUE (source, external_id)` is the projector's upsert target and the stable
identity later enrichment/override tables will reference. It is *not*
`(account_id, external_id)`: the account can be re-keyed by a re-import, the
source's own id cannot.

```
projection_offset(topic TEXT NOT NULL, partition INT NOT NULL,
                  next_offset BIGINT NOT NULL, updated_at TIMESTAMPTZ NOT NULL,
                  PRIMARY KEY (topic, partition))
```

**Dropped**: the legacy `transactions`, `categories`, `transaction_categories`,
`mandate_categories` entities are untouched by this iteration (iteration 2
redesigns categories); leave them alone rather than half-migrating them. The
`db_importer` and `categorize` bins that read the old shapes are updated only
as far as "still compiles" — `categorize` is rewritten in iteration 2.

---

## 4. Authentication

**Hashing.** `argon2` crate, Argon2id, default `Params` (m=19456 KiB, t=2,
p=1), per-password random salt via `OsRng`, PHC-string in `password_hash`.
Passwords are `SecretString` (`secrecy`) end-to-end — never a bare `String`,
never logged, never in a `Debug` impl.

**Sessions.** On successful login: 32 random bytes from `OsRng`, base64url —
that string is the cookie value and is never stored. Postgres stores
`sha256(token)` in `user_session.token_hash`. TTL 30 days (`expires_at`), with a
sliding `last_seen_at` refresh at most once per hour (avoids a write per
request). Logout deletes the row. Expired rows are pruned opportunistically on
login.

**Cookie.** Name `fr_session`; `HttpOnly`, `SameSite=Lax`, `Path=/`,
`Max-Age` = TTL, `Secure` controlled by `APP_cookie_secure` (default `true`;
the local dev `.env` sets `false` for plain-HTTP localhost). Set via
`ctx.append_http_header("set-cookie", …)` from the `login`/`logout` resolvers,
which the actix integration propagates onto the HTTP response.

**CSRF.** Three layers, all cheap: `SameSite=Lax` (blocks cross-site POST
cookie attachment), GraphQL requires `Content-Type: application/json` (not a
simple-request content type, so a cross-origin form cannot forge it), and the
`Origin` header — when present — must be in `APP_allowed_origins`. The current
`Cors::default().allow_any_origin()` **must go**: credentialed CORS and `*` are
incompatible, and allowing any origin with cookies would be the whole
vulnerability. Replace with an explicit allow-list from `APP_allowed_origins`
(comma-separated) plus `supports_credentials()`.

**Request context.** Actix extracts the cookie, looks the session up
(`token_hash` + `expires_at > now`), and injects `Option<AuthenticatedUser {
user_id, username, account_ids }>` into the async-graphql context via
`.data()` on the request. `account_ids` is loaded per request from
`user_account` (one query, ≤ a handful of rows).

`graphql/current_user.rs` is rewritten: `current_user(ctx)` returns the injected
user or a `NotAuthenticated` GraphQL error; `scoped_account_ids` keeps its
current signature and tests, now operating on `Uuid`. **Every** resolver that
reads `account`, `account_balance` or `transaction` goes through it — there is
no unscoped query path. `login` and `me` are the only fields callable
unauthenticated (`me` returns `null`).

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

Plus one convenience for the single-user reality: optional
`APP_projector_default_owner=<username>`. When set, the projector links every
newly created account row to that user in the same transaction. Unset (the
deployed default once more users exist) means accounts appear only after an
explicit `link`. This is the only place the projector touches `user_account`,
and it never unlinks.

---

## 5. GraphQL API

Shared contract between WP4 and WP5. Export with `graphql_schema_exporter`
and commit the SDL so the frontend can type against it.

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
}

type Mutation {
  login(input: LoginInput!): Me!
  logout: Boolean!
}

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
}

enum Direction { INCOME, SPENDING }
enum Granularity { DAY, WEEK, MONTH }

input PageInput { limit: Int! = 50, offset: Int! = 0 }   # limit clamped to 1..=200

type TransactionPage {
  items: [Transaction!]!
  totalCount: Int!
  limit: Int!
  offset: Int!
}

type CashflowSummary {
  buckets: [CashflowBucket!]!
  total: CashflowTotals!
  currency: String!
}

type CashflowBucket {
  start: Date!             # inclusive
  end: Date!               # inclusive
  income: Decimal!         # sum of amount > 0, positive
  spending: Decimal!       # sum of |amount| for amount < 0, positive
  net: Decimal!            # income - spending
  transactionCount: Int!
}

type CashflowTotals { income: Decimal!, spending: Decimal!, net: Decimal!, transactionCount: Int! }
```

Semantics and edge cases:

- `transactions` orders by `bookingDate DESC, externalId DESC` — a stable tie-break,
  otherwise paging through a day with many transactions can repeat or skip rows.
- `cashflowSummary` requires a bounded range: `startDate` and `endDate` must
  both be set, else a validation error. Unbounded aggregation over a growing
  log has no sensible bucket list.
- Buckets are **dense**: periods with no transactions are returned with zeros,
  so the chart has no gaps. `WEEK` starts Monday (ISO). `MONTH` buckets are
  calendar months, clipped to the requested range at both ends.
- Aggregation runs in Postgres (`date_trunc` + `SUM(…) FILTER (WHERE …)`), not
  in Rust; dense-filling of empty buckets is done in Rust (pure, unit-tested).
- `currency`: iteration 1 assumes a single currency across an account set and
  returns the first one seen (`EUR`). If an account set mixes currencies, the
  summary is still computed and the server logs a warning — no conversion, no
  error. Multi-currency is a later problem.
- Scoping errors: a `UUID` in `accountIds` the caller cannot see is an error
  (not a silent drop), matching today's `scoped_account_ids`. A caller with no
  linked accounts gets empty lists and zero totals, not an error.
- Unauthenticated access to anything but `me`/`login` returns a GraphQL error
  with `extensions.code = "UNAUTHENTICATED"`; the frontend keys its redirect
  off that code.
- `login` with bad credentials returns `extensions.code = "INVALID_CREDENTIALS"`
  with the same message and the same latency for unknown-user and wrong-password
  (hash a dummy PHC string when the user does not exist) — no user enumeration.

---

## 6. Frontend

**Same-origin GraphQL proxy.** The browser must send a cookie with every
GraphQL call; a cross-origin cookie (`localhost:5173` → `localhost:8080`) needs
`SameSite=None; Secure`, which does not work over plain-HTTP localhost. So
SvelteKit proxies: `src/routes/api/graphql/+server.ts` forwards the POST body
and the `cookie` header to the backend (private `GRAPHQL_URL`, default
`http://localhost:8080/graphql`) and relays `set-cookie` back.
`graphqlClient.ts` points at `/api/graphql` with `credentials: 'include'`.
`PUBLIC_GRAPHQL_URL` is retired; `.env.tower` sets `GRAPHQL_URL` instead. The
`filterSerializedResponseHeaders` note in `finreport-fe/CLAUDE.md` still
applies and must be updated, not removed.

**Auth handling.** `hooks.server.ts` resolves `me` once per request (cookie
forwarded), stores the result in `event.locals.user`, and
`+layout.server.ts` exposes it. A `load()` in `(app)/+layout.server.ts`
redirects to `/login?redirectTo=…` when `locals.user` is null; `/login` is
outside that group. Login is a form action → `login` mutation → relay
`set-cookie` → redirect.

**Routes**

- `/login` — username/password form, generic error message, no "username not
  found".
- `/` — dashboard. Period selector (This month / Last month / Last 3 months /
  This year / Custom range) + granularity (day/week/month, defaulting to the
  range: ≤ 31 days → day, ≤ 26 weeks → week, else month). Totals row
  (income, spending, net). Chart. Transaction list (paged, 50/page, newest
  first) filtered by the same period and by clicking a bucket.
- `/transactions` — the existing list page, re-pointed at the new SDL.

**Charting: `chart.js`.** It is already a dependency of `finreport-fe`, it is
framework-agnostic (a `<canvas>` + a Svelte 5 `$effect` to update it is ~30
lines), and a grouped bar chart of income/spending per bucket with a net line
is exactly what it is good at. `layerchart` was considered and rejected for
this iteration: adding a second charting dependency plus LayerCake concepts to
get a bar chart, when a working one is already installed, is cost without
payoff. Revisit if iteration 2's category views need composable layers.

Period math (bucket labels, default granularity, range presets) goes in
`src/lib/period.ts` as pure functions — that is what vitest covers (see
`finreport-fe/CLAUDE.md`: no component-render test setup).

---

## 7. Local dev environment

The whole stack must be demoable with no Comdirect credentials and no network
access to the bank or to `kafka.lab.anydef.de`.

**`docker-compose.local.yml`** gains:

- `finreport-redpanda`: `redpandadata/redpanda`, single node, `--mode dev-container`,
  `--smp 1`, advertising `127.0.0.1:19092` externally and
  `finreport-redpanda:9092` internally, with a healthcheck
  (`rpk cluster health`).
- `finreport-redpanda-init`: one-shot, `rpk topic create` for the four topics
  with the same partitions/cleanup policies as `terraform/kafka/main.tf`
  (1 partition, RF 1; `compact` for account/transaction/watermark, `delete` +
  `retention.ms=-1` for balances). This file and the Terraform module must be
  kept in step — a drift here means local behaves unlike deployed.

**`just` recipes**

| Recipe | Does |
|---|---|
| `dev-up` | compose up Postgres + Redpanda + topic init, `--wait` |
| `dev-down` | compose down |
| `dev-projector` | `cargo run -p webapp --bin projector` against local PG + `127.0.0.1:19092` |
| `seed-user` | `user-admin create-user --username dev` (password `dev` from env), idempotent |
| `seed-events` | `cargo run -p webapp --bin fixture-replay -- finreport-rs/webapp/fixtures` |
| `dev-demo` | `dev-up` → `seed-events` → run migrations → `seed-user` → print next steps |
| `dev-reset` | truncate the read model + `projection_offset`, so the next projector run replays |

`redpanda-console` keeps its existing default but is documented with
`just redpanda-console 127.0.0.1:19092` for the local broker.

**`fixture-replay`** (`webapp/src/bin/fixture_replay.rs`): reads JSON files from
a directory — `accounts.json`, `balances.json`, `transactions.json`, each an
array of raw payloads — and publishes each element verbatim with the §2.2
headers (`source=comdirect`). Fixtures are **synthetic but realistically
shaped** (including fields the structs do not model, so the raw-payload
promise is actually exercised), span ~6 months across 2 accounts, and contain
income and spending. They double as the integration-test corpus; no real bank
data is ever committed.

---

## 8. Testing

| Area | Level | What |
|---|---|---|
| Mappers | unit | Each `map_*` against fixture payloads: field mapping, missing optional fields, unparseable date → `MapError`, unmodelled fields survive into `raw_payload`, sign convention |
| Bucketing | unit | Dense bucket generation per granularity: empty range, single day, month clipped at both ends, ISO week boundaries, DST-free date-only math |
| Auth | unit | Argon2 hash/verify round-trip, wrong password rejected, token hashing, session expiry boundary |
| Scoping | unit | `scoped_account_ids` (existing tests, ported to `Uuid`) + "no linked accounts" |
| Projector | integration | testcontainers Postgres + Redpanda: replay fixtures → assert row counts/values; replay the **same** records twice → assert byte-identical table state (idempotency); kill mid-batch → restart → no duplicates, no gaps |
| GraphQL | integration | Real schema over testcontainers Postgres seeded by the projector: unauthenticated access denied, cross-user account access denied, pagination stability, `cashflowSummary` totals match a hand-computed fixture sum |
| FE logic | vitest | `src/lib/period.ts`, amount/row formatting, `cashflowSummary` → chart dataset shaping |
| FE smoke | Playwright | login → dashboard renders a chart canvas and ≥1 transaction row → logout → redirected to `/login` |

Integration tests use `testcontainers` as a dev-dependency and are gated behind
a `integration` cargo feature so `just test` stays fast and offline; a new
`just test-integration` runs them. The Playwright smoke runs against
`just dev-demo` (seeded fixtures), not against a live bank.

---

## 9. Work packages

Six packages. The two shared contracts — the **header envelope (§2.2)** and the
**SDL (§5)** — are frozen by this document, so packages that depend on them can
start immediately against the contract rather than against each other's code.

### WP1 — Event contract & importer cutover
Headers (`source`, `source_account_id`, `schema_version`) in `RecordMeta`;
remove all Postgres writes and the DB dependency from `import_transactions`;
make `APP_kafka_brokers` required there; publish failures become `error` +
watermark not advanced; `fixture-replay` bin + the fixture corpus;
`docs/kafka-migration.md` updated to describe phase 2 (it currently says
"Postgres stays the source of truth").
*Depends on:* nothing. *Blocks:* WP3 (contract only — already written here),
WP6 (fixture format).
*Done when:* the importer compiles without `sea-orm`, a local run against
Redpanda produces records carrying all required headers, `fixture-replay`
populates the four topics, and `rpk topic consume` shows unmodified payloads.

### WP2 — Schema, entities, auth core, CLI
Migrations from §3 (users, membership, sessions, reshaped read model,
`projection_offset`); regenerated entities; Argon2id hashing + session
create/verify/revoke module; `user-admin` bin; `APP_cookie_secure`,
`APP_allowed_origins`, `APP_projector_default_owner` in `utils::settings`.
*Depends on:* nothing. *Blocks:* WP3, WP4.
*Done when:* `make migrate` is clean on an empty and on an existing DB,
entities regenerate, `user-admin create-user`/`link` work against local PG, and
unit tests cover hash/verify + session expiry.

### WP3 — Projector
`projector` bin, `SourceMapper` trait + `ComdirectMapper`, batch consume →
map → upsert → offset commit in one transaction, stub-account handling,
poison-record skip, retry/exit policy, optional default-owner linking.
*Depends on:* WP2 (tables/entities), WP1's header contract (§2.2).
*Blocks:* WP4's integration tests and WP5's demo data.
*Done when:* `just dev-demo` fills the read model from fixtures, a second
replay changes no rows, `dev-reset` + replay reproduces the same state, and a
mid-run kill leaves no duplicates.

### WP4 — GraphQL API & HTTP surface
Cookie extraction + auth context, `login`/`logout`/`me`, rewritten
`current_user`, `accounts`/`transactions`/`cashflowSummary` resolvers against
the new tables, dense bucketing, `Decimal`/`Date`/`UUID` scalars, CORS
allow-list + credentials, `Origin` check, exported SDL committed.
*Depends on:* WP2. Uses WP3's output for integration tests (can be unblocked
with hand-seeded SQL).
*Done when:* the SDL matches §5, unauthenticated/cross-user access is denied by
test, `cashflowSummary` totals match a fixture-derived expectation, and the
login cookie round-trips in a real HTTP test.

### WP5 — Frontend
`/api/graphql` proxy route, `hooks.server.ts` + layout guard, `/login`,
dashboard (period selector, totals, chart.js chart, paged transaction list),
`src/lib/period.ts`, updated `/transactions`, `finreport-fe/CLAUDE.md` updated
for the proxy + retired `PUBLIC_GRAPHQL_URL`.
*Depends on:* the SDL (§5) only — may start against a mocked proxy response.
*Done when:* `npm run check` and `npm run lint` pass, vitest covers the period
helpers, and the Playwright smoke passes against `just dev-demo`.

### WP6 — Local dev stack & test harness
Redpanda + topic-init services in `docker-compose.local.yml` (kept in step with
`terraform/kafka`), `dev-up`/`dev-down`/`dev-projector`/`seed-user`/
`seed-events`/`dev-demo`/`dev-reset` recipes, testcontainers harness + the
`integration` feature + `just test-integration`, deployed-compose env updates
(importer loses `APP_database_url`, gains required `APP_kafka_brokers`;
new `finreport-be-projector` service with `APP_database_url` +
`APP_kafka_brokers`), root `CLAUDE.md` updated.
*Depends on:* WP1 (fixture format), WP3 (projector bin name/flags).
*Done when:* a clean checkout reaches a working, logged-in dashboard with
seeded data via documented commands, with no bank credentials and no access to
the central broker.

**Parallelism:** WP1, WP2, WP5, and the compose half of WP6 can run
concurrently from the start. WP3 and WP4 start once WP2's migrations land.
WP4 and WP5 touch disjoint trees. Only WP2 and WP3 both touch
`finreport-rs/migration` and `entity` — WP3 must not add migrations.

---

## 10. Assumptions & open questions

**Assumptions made (decided here, not escalated)**

1. Current Postgres contents are expendable; the read model is rebuilt from
   Kafka plus a full Comdirect re-walk (§2.8).
2. Single currency (EUR) across the board; mixed-currency account sets are
   summed without conversion and logged.
3. Offset storage in Postgres rather than Kafka consumer-group offsets, for
   transactional commit with the projected rows.
4. One projector process, single instance. All topics are 1 partition, so there
   is nothing to scale out, and two instances would fight over
   `projection_offset`. Not enforced with a lock in this iteration.
5. Offset-based pagination, not cursors. A household's transaction list does not
   reach the depth where offset paging hurts.
6. `chart.js` over `layerchart` (§6).
7. Session TTL 30 days, sliding; no "remember me" distinction.

**Open questions (do not block iteration 1)**

- Does Comdirect actually return transactions newest-first? Still unverified;
  the runtime guard in `comdirect-rs` remains the only protection and must not
  be removed.
- Joint-account deduplication across two logins (same IBAN, two `accountId`s) —
  deferred to iteration 3, where internal-transfer detection needs account
  identity anyway.
- Whether the deployed broker's retention/compaction settings make a
  from-zero replay of `finreport.transaction` reliable indefinitely, now that
  the read model depends on it. Compaction on `reference` preserves one record
  per transaction, which is sufficient today; revisit if a topic ever gets a
  retention limit.
- Whether `account_balance` should keep a full observation history rather than
  one row per account-day once a balance chart exists (iteration 4).
