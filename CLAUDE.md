# finreport

Personal-finance reporting: a Rust Comdirect API client + GraphQL backend, feeding a SvelteKit frontend.

## Layout

- `finreport-rs/` — Cargo workspace (resolver 3). Members:
  - `webapp` — actix-web + async-graphql server; binaries in `webapp/src/bin/` for one-off jobs (`import_transactions`, `categorize`, `db_importer`, `init_session`, `graphql_schema_exporter`)
  - `comdirect-rs` — Comdirect REST API client
  - `categorizer` — transaction categorization logic (taxonomy lives in `prompts/taxonomy.json`)
  - `entity` — sea-orm generated entities (do not hand-edit; regenerate, see below)
  - `migration` — sea-orm migrations
  - `utils` — shared settings/config (`utils::settings::Settings`)
  - `amazon-orders` — Amazon order CSV parser + order-to-charge matcher, pure functions (see "Amazon purchase orders" below)
  - `c24-csv` — C24 bank CSV export parser, pure functions (see "C24 CSV import" below)
- `finreport-fe/` — SvelteKit frontend, talks to the GraphQL backend. See `finreport-fe/CLAUDE.md` for frontend-specific commands.
- `prompts/` — LLM categorization prompt (`categorize.txt`) + category taxonomy
  (`taxonomy.json`, what `category-seed` publishes). The taxonomy lives in
  `taxonomy.json` alone; an older `categories.json` was deleted on 2026-10-23
  (unread since May 2026, pre-`slug`/`kind` shape, subcategory names disagreeing
  with the live ones). `finreport-fe/src/lib/graphql/mocks/categories.json` is a
  different, live file — frontend mock data, not the taxonomy.
- `docs/` — Comdirect Postman collection, plus `docs/multi-agent-setup.md` tracking this repo's multi-agent prep
- `terraform/` — deploy infra (Portainer stack)
- `.gitea/` — CI

## Conventions

- Logging via `tracing`, not `log`/`println!`.
- Secrets wrapped in `secrecy` (`ExposeSecret`), never held as plain `String`.
- Env vars are `APP_*`, loaded through `utils::settings::Settings`; local secrets come from 1Password via `.env.tpl` (`op run --env-file .env.tpl -- ...`).
- The root `README.md`'s `sqlx-cli` install/migrate instructions are stale — this project uses **sea-orm-cli**, not sqlx-cli (see Commands below).

## Commands

Run from repo root unless noted.

```bash
# Rust workspace
just test                    # cargo test across finreport-rs (unit only, offline)
just test-integration        # testcontainers-backed suite (Postgres + Kafka); needs Docker
just lint                    # cargo clippy across finreport-rs (used by CI)
just db-up / just db-down    # local Postgres via docker compose
just dev-be                  # run the GraphQL backend locally (see "Running locally" below)
just dev-be-tower            # run the GraphQL backend locally against the tower (deployed) Postgres — see warning below
just import-local [--account <key>]   # run the transaction importer against local Postgres

# Local dev stack: Postgres + Redpanda + topic-init (phase 2, see Kafka section below)
just dev-up                  # compose up Postgres + Redpanda + topic-init, healthy
just dev-down                # compose down (drops the Postgres volume too)
just dev-reset               # truncate the read model + labeling projections + projection_offset so the next projector/labeler run replays from scratch
just dev-projector [ARGS]    # cargo run the projector against the local stack; pass --until-caught-up to exit at the log end
just dev-labeler [ARGS]      # cargo run the labeler (iteration 2) against the local stack; pass --until-caught-up; refuses to start if the projector lags
just seed-user                # create the demo `dev` user (idempotent)
just seed-events               # replay the fixture corpus onto the local Redpanda
just seed-categories            # publish the taxonomy onto finreport.category (idempotent)
just dev-demo                 # the whole demo: dev-up → migrate → seed-user → seed-events → seed-categories → alternate projector/labeler until quiescent → link accounts → assert a learned rule + a needs-review label exist
just redpanda-console [broker] # Redpanda Console UI (http://localhost:8090); defaults to the central broker, pass 127.0.0.1:19092 for the local stack

# DB (run inside finreport-rs/, needs .env with APP_database_url)
make migrate                 # sea-orm-cli migrate
make generate-entities       # regenerate entity/src/entities from DB schema
make generate-migrations TARGET=<name>

# Frontend — thin `just` wrappers around the npm scripts in finreport-fe/CLAUDE.md
just dev-fe                  # local profile
just dev-fe-tower            # tower profile
```

## Running locally

Each component starts independently — no need to bring up the full docker-compose stack for dev work. The whole demo (steps 1–3 below, via `just dev-demo`) needs **no Comdirect credentials and no network access** to the bank or to the central Kafka broker.

1. **Postgres + Redpanda**: `just dev-up` brings up Postgres, a single-node Redpanda, and a one-shot topic-init job (the same four topics/configs as `terraform/kafka/main.tf`) under `docker-compose.local.yml`, all with overridable ports (`FINREPORT_PG_PORT`/`FINREPORT_KAFKA_PORT`, defaulting to `5432`/`19092`). Stop with `just dev-down`; `just dev-reset` truncates the read model so a fresh projector run starts over without touching `app_user`/`user_account`/`user_session` (those links survive by design — see §2.3 of the spec). `just db-up`/`just db-down` still work if you only need Postgres (e.g. for `dev-be`, no Kafka).
2. **Backend**: `just dev-be` — no `.env` needed; local defaults come from `local_env` in the `justfile` (override any `APP_*` from your shell). Runs migrations automatically and serves GraphQL on `:8080`. `dev-be` needs no Comdirect creds at all — it only serves GraphQL against the DB and never calls the Comdirect API. Real imports go through `just import-local` instead, which pulls real creds from 1Password.
3. **Demo data, no bank needed**: `just dev-demo` seeds a `dev` user, replays the fixture corpus onto the local Redpanda, publishes the category taxonomy, then alternates the projector and labeler until a round produces no new rows (max 5, else it fails loudly), links the seeded accounts to `dev`, and asserts a learned rule and a needs-review label exist — the fixtures are synthetic but realistically shaped, spanning ~6 months across 2 accounts, with a repeated merchant that reaches the rule-learning threshold under the fake LLM provider (no API key needed). Or drive the steps yourself: `just seed-user`, `just seed-events`, `just seed-categories`, `just dev-projector --until-caught-up`, `just dev-labeler --until-caught-up`.
4. **Frontend**: `just dev-fe` (or `cd finreport-fe && npm run dev` directly) — see `finreport-fe/CLAUDE.md` for the local/tower profile switch.

## Comdirect logins (importer)

The importer handles multiple Comdirect logins in **one process, one task per
login**. Each task runs its own state machine, so a login approves its own
push-TAN and re-bootstraps its own stale session without holding up the others;
a login that fails for good ends only its own task, and the process exits
non-zero once every account has given up.

Logins are configured as numbered accounts and read by `utils::settings`:

```
APP_accounts__0__name / __client_id / __client_secret / __zugangsnummer / __pin
APP_accounts__1__...
```

- `import-transactions` imports every configured account; `--account <key>`
  narrows a run to one (the account **key**, never the `__name` label), which is
  mostly useful locally. `init-session` drives a single login interactively, so
  it requires the flag when several accounts are configured.
- `__name` is a free-text, human-readable label for the login, persisted to
  the nullable `account.account_name` on every account it imports (migration
  `m20260820_000001_account_name`, exposed as the nullable `Account.accountName`
  in GraphQL). It is **display only**: nothing resolves an account through it,
  it has no default (unset stays NULL rather than borrowing the key), and it is
  refreshed on re-import so renaming a login propagates on its next run.
  Accounts are referenced by the account key in config and by `account_id` in
  the DB — both stable, unlike a label someone may reword.
- Each account persists its tokens separately: `APP_save_file_path` with the
  key spliced in (`.session.0.json`), overridable per account via
  `APP_accounts__<key>__save_file_path`.
- The older flat form (`APP_client_id`/`APP_client_secret`/`APP_zugangsnummer`/
  `APP_pin`, labelled with `APP_account_name`) still works as a single account
  named `default`, and is ignored as soon as any `APP_accounts__*` var is set.
- Deployment: the single `finreport-be-importer` service in
  `docker-compose.yml` — the deployed stack file, which runs prebuilt
  `${DOCKER_REGISTRY}/finreport-be` images with static LAN IPs, **not** the
  `build:`-based `docker-compose.local.yml`. Adding a login means a
  `TF_VAR_app_account_<n>_*` block in `.env.tpl` (terraform flattens those into
  `APP_accounts__<n>__*`), a matching variable block in `terraform/variables.tf`,
  and the env block in the compose file. No new container, address or port.
- **`.env.tpl` values must each be exactly one `op://` reference.** The file is
  read three ways — `op run` (`just import-local`), `op inject` (local
  `just deploy`), and `1password/load-secrets-action` in CI — and only that form
  works in all of them. Nothing there is shell-expanded in CI, so no value may
  refer to another; that is why terraform takes flat per-account strings rather
  than one JSON-encoded list. Non-secret values (like `__name`) belong in
  `docker-compose.yml`, not in `.env.tpl`.
- An account visible from two logins is imported idempotently *as long as
  Comdirect reports the same `accountId` for both*: accounts and balances
  conflict-`do_nothing`, transactions upsert on `reference`. If the same IBAN
  ever arrives under two different `accountId`s, the second insert trips
  `account`'s unique IBAN index — logged as an error, and its balances and
  transactions then fail their foreign keys. Noisy, but not corrupting.

## Event log (Kafka) — migration phase 2

Kafka is now the source of truth for the read model: the importer publishes
only (it no longer writes to Postgres itself, and no longer takes
`APP_database_url` at all — `APP_kafka_brokers` is a hard requirement, not
best-effort), and a new `projector` binary consumes the four topics and
builds the Postgres read model (`account`, `account_balance`, `transaction`,
keyed by deterministic UUIDs so replays are idempotent). See
`docs/specs/iteration-1.md` §2.2 (envelope contract), §2.3 (projector design)
and §2.8 (the legacy-backfill cutover runbook that reshaped the old
`account`/`account_balance`/`account_transactions` tables into
`legacy_account`/`legacy_account_balance`/`legacy_account_transactions`) for
the full design.

- **Broker**: `kafka.lab.anydef.de:9092`, plaintext, for the deployed stack —
  central homelab infrastructure this repo does not own or deploy; finreport
  only owns its own topics on it (`terraform/kafka`). **Locally**,
  `just dev-up` runs a single-node Redpanda under `docker-compose.local.yml`
  with the same four topics/configs, so the whole demo needs no network
  access to the central broker at all (see "Running locally" above).
- **Topics** (`finreport.account`, `.account-balance`, `.transaction`,
  `.import-watermark`) are managed by Terraform as the `terraform/kafka` child
  module of the existing root module — same state, same `just deploy` — for
  the central broker, and mirrored by `docker-compose.local.yml`'s
  `finreport-redpanda-init` one-shot job for local dev; the two must be kept
  in step by hand (see the comment in `terraform/kafka/main.tf`). Each
  topic carries `prevent_destroy`: CI applies unattended, and an edit that
  would *replace* a topic (rename, fewer partitions) deletes its events, so
  those fail the apply instead. Retention/cleanup-policy edits apply in place.
  Every plan of this module refreshes finreport's topics, so plans need the
  broker reachable; if it's down, `terraform apply -target=module.portainer_stack`
  deploys the app without touching Kafka.
- **Payloads are the raw Comdirect JSON, byte-for-byte**, with one documented
  exception: the legacy-backfill reconstruction (§2.8), which publishes
  `origin=legacy-backfill` records only for keys with no existing raw record,
  and never touches a key the importer has already written. Our own metadata
  (which login, when, schema version, origin) rides in Kafka **headers** (the
  full envelope contract, §2.2) so the value stays exactly what the bank
  returned whenever `origin=source`.
- **Publishing is best-effort against the broker, not against the import**:
  `APP_kafka_brokers` unset is now a startup error for the importer (it has
  no other output), and a publish failure is now data loss, not a degraded
  side-channel — the import loop logs at `error` and does **not** advance
  that account's watermark for the cycle (advancing it past an unpublished
  transaction would lose it permanently), so the next cycle re-fetches and
  retries instead of silently skipping ahead.
- **The deployed stack** (`docker-compose.yml`) gained a `finreport-be-projector`
  service alongside `finreport-be-importer` — same images, same Kafka broker,
  consuming instead of producing.
- **Inspecting it**: `just redpanda-console` runs Redpanda Console locally
  (http://localhost:8090) against the central broker by default — local
  container only, nothing deployed. Pass a different broker/port to aim it
  at the local stack instead: `just redpanda-console 127.0.0.1:19092`.
- **Resume points**: at startup each importer account reads its watermark
  from the compacted `finreport.import-watermark` topic and only fetches
  transactions newer than it. The Comdirect bank-account transactions
  endpoint has **no date filter**, so this is client-side early-stop
  pagination, which assumes newest-first ordering — an assumption
  `comdirect-rs` guards at runtime and falls back to a full walk when
  violated. That guard is the riskiest part of the design; do not remove it.
  The projector has its own, separate resume point: `projection_offset`
  (one row per topic-partition) in Postgres, so a projector restart or
  `just dev-reset` resumes/replays independently of the importer's watermark.

## Event log (Kafka) — migration phase 3 (labeling pipeline, iteration 2)

A second always-on consumer, `labeler`, joins the projector: it resolves a
category for every transaction through the precedence chain (user override >
rule > LLM cache > LLM), publishing its own records rather than writing
Postgres directly — the labeling read model is a projection like every other
table. See `docs/specs/iteration-2.md` §2.2–§2.9 for the full design (rules,
learning, cost guard, compare-before-publish) and `docs/architecture.md` for
the label-resolution and learned-rule-lifecycle diagrams.

- **New topics**: `finreport.transaction-label`, `.llm-cache`, `.user-label`,
  `.rule`, `.category` (all compacted, `prevent_destroy`, same posture as the
  iteration-1 topics) and `finreport.label-request` (delete-cleanup, 7-day
  retention, **not** `prevent_destroy` — a work queue, not state). Managed by
  `terraform/kafka/main.tf` for the central broker and mirrored by
  `docker-compose.local.yml`'s `finreport-redpanda-init` for local dev — keep
  both in step by hand, as with the iteration-1 topics.
- **Offsets**: the labeler has no consumer group; like the projector, it
  keeps its position in Postgres `projection_offset`, under topic keys
  suffixed `@labeler` — distinct rows, same table, same transactional commit
  as the labels it publishes.
- **Startup guard**: the labeler refuses to start (exit non-zero) while the
  projector's offsets trail the broker's high watermark by more than
  `APP_labeler_max_projection_lag` records (default 0) — it resolves against
  the projection, so running ahead of the projector means re-deriving answers
  for transactions that already have one. Deployed, this is a one-time
  startup check between two independent always-on services; locally,
  `just dev-demo` satisfies it by alternating `projector --until-caught-up`
  and `labeler --until-caught-up` instead of running them concurrently.
- **LLM provider**: `APP_llm_provider` defaults to `fake` — nothing in the
  repo calls a paid API unless it's set to `anthropic`, `ollama` or `openai`.
  An optional `ollama` Docker Compose profile (`finreport-ollama`, off by
  default) is available for local testing against a real small model; start
  it with `docker compose -f docker-compose.local.yml --profile ollama up
  finreport-ollama -d`.
- **Anthropic key wiring**: `.env.tpl` has exactly one line,
  `TF_VAR_anthropic_api_key="op://HomeLab/finreport/anthropic api/api_key"` — a
  single `op://` reference, nothing shell-expanded, per the `.env.tpl` rule
  above. `terraform/variables.tf` declares `anthropic_api_key` `sensitive =
  true` (default `""`, since the key is only needed once a deploy actually
  sets `APP_llm_provider=anthropic`); the deployed `docker-compose.yml` passes
  it to the new `finreport-be-labeler` service as `APP_anthropic_api_key`.
  Non-secret knobs (provider, model) live in `docker-compose.yml`, never in
  `.env.tpl`. The key now reaches the stack via `terraform/main.tf`'s
  `module "portainer_stack"` `extra_env` (`APP_anthropic_api_key = var.anthropic_api_key`),
  the same mechanism `POSTGRES_PASSWORD` uses.
- **Deployed**: a new `finreport-be-labeler` service alongside
  `finreport-be-projector` — same image, same Kafka broker, static LAN IP
  `192.168.100.49`, no new port.

## Transaction links (reimbursements)

A user-declared tie between transactions that offset one another, on its own
compacted topic `finreport.transaction-link` (keyed by link id, `prevent_destroy`,
mirrored in `terraform/kafka/main.tf` and `docker-compose.local.yml`), projected
into `transaction_link` + `transaction_link_member` by `projection::links`. It is
**not** the detector's `transaction-insight` (so no detector run can overwrite it)
and not the transfer flag. A link has a `kind` (only `reimbursement` today) and
members with a role (`expense` / `offset`); a pair is the common case, but one
expense with several reimbursements and one reimbursement over several expenses
are each a single link. Amounts are never stored: `links::compute` derives the
offset from the members' own amounts, `min(offsets, expenses)` shared pro rata,
the excess of an over-reimbursement reported as `surplus`.

- Linking never rewrites a period. `Transaction.link` annotates every row,
  `reimbursementSummary(filter)` gives the netted figure separately, and
  `TransactionFilter.reimbursements` (`INCLUDE` default | `EXCLUDE` | `ONLY`)
  concerns the offsetting side only. Removing a link is a tombstone and restores
  the unlinked view exactly.
- A transaction is on at most one link (enforced at write time, not in the
  projection, so a replay cannot fail on it).
- The list resolvers prefetch `Transaction.link` per page (`graphql::links::prefetch_page`).

## C24 CSV import

The `c24-csv` crate parses C24's CSV account export. **Parser, topics, wire
records, projection and the upload mutation exist; the frontend and the
labeler do not.** C24 records
go on **their own topics** `finreport.c24-account` and
`finreport.c24-transaction` (compacted, `prevent_destroy`, keyed by the account
number / the row's derived `external_id`, mirrored in `terraform/kafka/main.tf`
and `docker-compose.local.yml`), not on `finreport.transaction` (whose
byte-for-byte-Comdirect-JSON promise the projector relies on). Two topics
because `transaction.account_id` is a real FK; there is **no** balance topic
because the export has no balance column. They project into the same
`account` / `transaction` tables with `source = "c24"`, via
`projection::c24::C24Mapper` registered at `("c24", "source")`.

- **`webapp::kafka::c24::{C24AccountRecord, C24TransactionRecord}` are now a
  frozen wire contract.** Their serde shape is the stored payload: renaming a
  field needs a `schema_version` bump and a migration path, not just a passing
  test suite (`transaction_wire_shape_is_frozen` pins the key set).
  **`C24Row` is deliberately not the payload** - `from_row` is the one
  translation, so the parser stays free to be refactored and this does not
  repeat the `amazon_orders::Order` trap below.
- `C24Mapper` never constructs a value-carrying `MapError` variant and never
  forwards a serde message: every failure is `InvalidPayload` with a static
  string. `map_balance` always fails (nothing to fabricate).
- The record's `currency` is the literal `"EUR"` set in `from_row` (the export
  has no currency column; the parser drops the `€`). The account record's
  `iban` is `None` on purpose: the export's `IBAN` column is the
  *counterparty's*. C24's own `Kategorie`/`Unterkategorie` live in
  `raw_payload` only.
- **Labelled like any other source.** `finreport.c24-transaction` is in
  `LABELER_INPUT_TOPICS` and shares the transaction handler with
  `finreport.transaction`; the `source` header, not the topic, is what tells
  them apart. The labeler holds no per-source code (§2.3).

Same shape and the same two binding properties as
`amazon-orders`:

- **A bad file is rejected whole, never row by row.** A half-imported bank
  statement is worse than a rejected one — the user cannot tell which lines are
  missing, and the gap reads as money that never moved.
- **No error message or log line carries a cell value.** Every cell is bank
  data (amount, counterparty, IBAN, purpose), so `ParseError` holds a column, a
  line and a *static* reason only. `csv::Error` is reduced to its class and
  position, because its own `Display` quotes the record it choked on.

**`uploadC24Transactions(files: [String!]!)`** (`graphql/c24_upload.rs`, mirrors
`uploadPurchaseOrders`): parse each file, merge, publish only what is new or
changed, project what reached the broker, link, report per file.

- **Not admin-gated, deliberately**: importing your own statement is your own
  data (boundary settled 2026-10-25). Do not add `require_admin`.
- **Publishes through `RecordMeta` + the ack-awaiting `EventPublisher::publish`,
  never `events::publish_event`**: that helper omits `source_account_id`, which
  `C24Mapper::map_transaction` reads off the envelope, so every record would
  skip on replay. The integration test feeds each scanned record back through
  the mapper.
- **Idempotent**: each record is compared with the stored row's `raw_payload`
  (ids are `transaction_uuid("c24", external_id)`); identical means unchanged
  and nothing is published. A changed `Kategorie`/`Kontoname` keeps the id and
  counts as updated. Projection calls `apply_account`/`apply_transaction`
  directly, not `process_batch` (which would write projector offsets).
- **Every imported account is linked to the caller** in `user_account`
  (idempotent, every upload), or the import would be invisible.
- **An account already linked to other users and not the caller refuses the
  whole upload** (`VALIDATION`): account ids derive from the account number
  alone, so otherwise knowing a number would grant access to its transactions.
- Errors and logs carry no cell, row or account number.

Format facts, all verified against a real export (14-column header
`Transaktionstyp,Buchungsdatum,Karteneinsatz,Betrag,Zahlungsempfänger,IBAN,BIC,Verwendungszweck,Beschreibung,Kontonummer,Kontoname,Kategorie,Unterkategorie,Bargeldabhebung`):

- **The export is ragged**: data rows carry 13 fields, the trailing
  `Bargeldabhebung` value simply absent. 13 and 14 are both accepted; any other
  count, or any header that is not exactly the 14 above in order, rejects the
  file.
- Amounts are quoted and German-formatted with a currency symbol
  (`"-1,00 €"`, `"150,00 €"`), parsed to `rust_decimal::Decimal`, never a float.
  The thousands separator (`"1.234,56 €"`) is **inferred, not observed**.
- `Karteneinsatz` (`dd.MM.yyyy HH:mm`) is empty on non-card rows;
  `IBAN`/`BIC`/`Verwendungszweck`/`Beschreibung` are empty on card rows. All
  optional. `Buchungsdatum` is `dd.MM.yyyy`.
- `Transaktionstyp` stays a `String`, not an enum: an unknown type must never
  reject the file.
- `Kategorie`/`Unterkategorie` are **C24's own guess — evidence, not a
  decision**. A later slice may feed them in as a hint at the bottom of the
  precedence chain; they must never override a user override or a rule.
- One file can carry several accounts (`Kontonummer` + `Kontoname`), so nothing
  assumes one.

**Identity, because the export has no transaction id.**
`content_key = SHA-256(kontonummer, buchungsdatum, karteneinsatz, betrag,
zahlungsempfaenger, iban, verwendungszweck, beschreibung, transaktionstyp)`
(parts joined with `\0`, as `kafka::envelope::uuid_v5_from_parts` does), and
`external_id = "<content_key>-<occurrence ordinal>"`.

- The ordinal exists because the real export contains byte-identical rows that
  are **genuinely distinct transactions** (one identical pair, and one identical
  triple, same `Karteneinsatz` and all). A plain content hash would collapse
  three into one and lose two.
- **The ordinal is scoped to `(kontonummer, buchungsdatum, content_key)` — the
  booking day, not the file.** C24 filters by whole days, so any export covering
  a day covers all of it, and a triple gets 1, 2, 3 in *every* export including
  that day. Scoping to the file would shift ordinals whenever the range changed
  and duplicate everything on re-import.
- Ordinals are assigned after a canonical sort, so row order in the file cannot
  affect them.
- **Accepted weakness, documented not hidden**: if C24 re-words a row's
  description, purpose or payee, it re-imports as a new transaction. Leaving
  the text out of the key instead would merge distinct transactions.

## Amazon purchase orders

The `amazon-orders` crate holds the CSV parser and the order-to-charge matcher
as pure functions (no database, Kafka or GraphQL), so the risky logic is
testable in isolation. Orders are **evidence about** transactions, never
transactions themselves - publishing one as a transaction would double-count
the purchase, since the bank line already exists.

Stored on the compacted topic `finreport.purchase-order`, keyed
`<user_id>:<source>:<order_id>` (`prevent_destroy`, mirrored in
`terraform/kafka/main.tf` and `docker-compose.local.yml`), projected to
`purchase_order` / `purchase_order_item` by `projection::purchase_orders`.
Revision-guarded last-writer-wins; a tombstone deletes the order and its items.

- **`amazon_orders::Order` is now a wire contract.** The record wraps it
  unchanged to avoid a translation layer, so its serde shape *is* the payload:
  renaming a field or a `PriceBasis` variant in the crate silently breaks
  every stored record. Any such change needs a `schema_version` bump and a
  migration path, not just a passing test suite.
- **The export's six recipient-address columns are never stored or logged.**
  The parser validates them in the header and then never reads them, `Order`
  and `Item` have no recipient fields, and an integration test smuggles
  `recipient_*` values in at record, order and item level then asserts no
  column name and no stored value matches. Keep that property.
- Poison-record logs carry the serde error's class, line and column only -
  never its text, which can quote payload values.
- The matcher applies a split only when exactly one assignment reconciles;
  otherwise it returns `NeedsReview` with a user-facing reason. The upload
  flow therefore needs a review surface, not just a success path.
- **Upload and review (`/admin/amazon`).** `uploadPurchaseOrders(files: [String!]!)`
  takes each CSV's full text (no file-upload plumbing here), parses it, merges
  files, and publishes + projects only orders that are new or changed; a
  re-upload of an unchanged order publishes nothing. **A bad file is rejected
  whole, never row by row** (an order spread over item rows would otherwise be
  stored with items missing); other files in the same call still import, and
  the result names each rejected file by position with the parser's
  column/line message. `purchaseOrderMatches` returns the match report in three
  queries (orders, items, Amazon-looking bank lines) with each charge's detail
  already in hand: no per-row resolvers. Uploading **splits nothing**.
- **Applying (`applyPurchaseOrderSplit` / `undoPurchaseOrderSplit`, the only
  Amazon code that changes transactions).** Per order, explicit, never from an
  upload. The server re-runs the matcher and applies only a still-`Apply`
  match; the client sends one category slug per item and **no amount**. A part
  is an amount and a category only (`transaction_split` has no description or
  tag column, and item titles are not categorised here), so the *user picks the
  category per item* and the title is kept in `purchase_order_split.parts`.
  `graphql/purchase_order_apply.rs` holds the pure decisions: `build_parts`
  (exact `Decimal` sum or refuse), `decide` (apply / already applied / skip),
  `undo_decision`. A transaction with **any** split or a pinned category is
  skipped and reported, because `splitTransaction` itself would overwrite both.
  `purchase_order_split` (PK = transaction id) is the provenance record: apply
  claims it first, so a double apply cannot both win, and undo removes a split
  only while it still equals the record, else it leaves the user's version.
  Undo goes through `unsplitTransaction`'s path, so tags, note and recurring are
  kept; the transaction falls back to its rule/LLM category. The table is not
  truncated by `just dev-reset`; a stale row is detected, never trusted.
  An `Apply` match always has items summing to the order total, so the
  shipping-and-adjustments remainder part exists in code but cannot occur until
  the matcher accepts orders with a remainder.
- Page grouping lives in `finreport-fe/src/lib/amazonView.ts`; the matcher's
  reason `message` is shown verbatim, only the group titles are the page's own.
  The mock is `graphql/amazonMock.ts`.

## Savings

Two separate things, both easy to get wrong:

**`kind: SAVING` categories.** A saving-kind category is money put aside, not
consumed. `categoryBreakdown` drops only `kind: TRANSFER` by default, so
savings rows come back unless a caller filters them out — and the dashboard
did exactly that, asking for `kind: EXPENSE` alone, which is why 64 labelled
savings transactions were invisible on it. The dashboard now fetches both
kinds and merges them (`mergeExpenseAndSavingBreakdowns`). **`share` is
computed per kind** in `graphql/breakdown.rs`, so each response's rows already
sum to 1: concatenating two responses raw prints percentages adding to 200%.
The merge helper recomputes `share` over the combined total. `/compare` still
defaults to `$kind: CategoryKind! = EXPENSE` and so still excludes savings.

**Savings accounts** (built: toggle at `/admin/accounts`, card on the dashboard;
mock mode keeps its flags in `graphql/savingsMock.ts`). The user marks
accounts as savings, per user, like a nickname — a reporting choice, not a
fact the bank reports. The figure is **net balance change**, decided
deliberately: transfers in, withdrawals out, interest or salary paid straight
in, and a payment made directly from a savings account all count. A movement
between two savings accounts counts as neither, since it changes no total.

- `transaction_insight.transfer_counterpart_id` holds the other side and the
  detector flags **both** rows, so count only the savings-account side and
  drop a row whose counterpart is also a savings account, or every internal
  transfer is counted twice.
- Money spent straight from a savings account is in the savings figure **and**
  in the spending card. The two are each correct but are **not** a partition;
  the card says so (`savingsView.ts`'s overlap note) rather than letting a
  reader add them up.
- A mixed-currency savings set makes `savingsSummary` return a `VALIDATION`
  error; `savingsCardState` turns that into an explanatory line, and any other
  error or a null summary renders nothing, so the dashboard never fails on it.
- `queries.test.ts` validates every operation in `queries.ts` against the frozen
  `schema.graphql`, so an operation cannot be added before the schema serves it.

## Backend database profiles

The backend can run locally against either Postgres:

- `just dev-be` → the local Postgres from `just db-up` (`127.0.0.1:5432`), config from `finreport-rs/.env`.
- `just dev-be-tower` → the **real, deployed** tower Postgres (`192.168.100.46:5432`, same LAN-reachable host the deployed `finreport-be` container uses). Everything except `APP_database_url` still comes from `finreport-rs/.env`; the real DB password is pulled live from 1Password via `op read` and never written to disk.

  **Be deliberate with this one.** `webapp`'s startup (`db/seaql.rs::init_db`) runs `Migrator::up()` unconditionally — pointing the local binary at tower means any migration that exists locally but isn't deployed yet gets applied to the live database the moment you run it. Don't run `dev-be-tower` with unreviewed/WIP migrations sitting in `finreport-rs/migration/`.

## Admin bootstrap

`webapp`'s startup (`webapp::auth::bootstrap`, called from `main.rs` right after the DB connection is made) creates/maintains a single admin `app_user` whenever `APP_admin_password` is set: creates it if missing, re-hashes the password if it no longer verifies (so a Terraform-driven rotation takes effect on the next restart), promotes an existing non-admin row with the same username, and links every account to it — same effect as `user-admin link --all`, but automatic and idempotent across restarts.

- `APP_admin_username` (default `admin`) / `APP_admin_password` (`SecretString`, `Option` — unset disables the whole bootstrap) are read through `utils::settings::Settings` like everything else.
- Local dev (`just dev-be`/`dev-demo`) never sets `APP_admin_password`, so bootstrap creates no admin there. Instead `just seed-user` seeds the `dev` demo user **as an admin** (`user-admin create-user --admin`): `isAdmin` is enforced server-side on the rules and category resolvers, so an unprivileged `dev` would be locked out of `/admin/rules` and `/admin/categories`. Plain `create-user` stays unprivileged.
- Repairing an existing install: `user-admin promote --username <u>` / `demote --username <u>` set and clear `is_admin` (idempotent; unknown user is an error). `list-users` marks admins with `[admin]`. There is deliberately no last-admin guard — `demote` may leave zero admins. In the deployed stack bootstrap only promotes `APP_admin_username`; any other user must be promoted by hand.
- In production the password is generated once by Terraform (`random_password.admin`, `terraform/main.tf`) and written to the 1Password item **"finreport admin"** (HomeLab vault, via `onepassword_item.finreport_admin`); it reaches the container as `APP_admin_password` through the same `extra_env` mechanism as `POSTGRES_PASSWORD`. Rotating it is just `terraform apply` (regenerates the random password and the 1Password item) followed by a `finreport-be` restart.
- The `onepassword` Terraform provider reads 1Password Connect credentials from the environment, not from a hardcoded value: `OP_CONNECT_HOST`/`OP_CONNECT_TOKEN` (or `OP_SERVICE_ACCOUNT_TOKEN`). CI's "Load secrets" step already receives these as action inputs (`.gitea/workflows/build-deploy.yaml`) and the "Deploy to Portainer" step re-exports them into its own `env:` block so `terraform` sees them too. A local `just deploy` gets them for free from `.build/build-tools/deploy-portainer.sh`/`deploy-terraform.sh`, which already `op read 'op://HomeLab/1password-connect/...'` whenever `OP_CONNECT_HOST` isn't already set and neither `GITHUB_ACTIONS` nor `GITEA_ACTIONS` is — no extra setup needed beyond having `op` signed in.

## Frontend backend-target profiles

The frontend can point at either backend, selected by Vite `--mode`:

- `just dev-fe` / `npm run dev` (default `development` mode) → `http://localhost:8080/graphql`, i.e. whichever backend you've started locally (step 2 above, `dev-be` or `dev-be-tower` — both bind `:8080`, so only run one at a time).
- `just dev-fe-tower` / `npm run dev:tower` (`--mode tower`, loads `finreport-fe/.env.tower`) → the *deployed* backend on the Unraid box directly, no local backend needed at all. Distinct from `dev-be-tower`: this one skips your local backend entirely and hits the deployed `finreport-be` container's GraphQL endpoint over the network.

Add more profiles by dropping a new `finreport-fe/.env.<mode>` file (setting `PUBLIC_GRAPHQL_URL`), a matching `dev:<mode>` script in `finreport-fe/package.json`, and a `dev-fe-<mode>` justfile wrapper.

## Deployed frontend

`finreport-fe` (adapter-node) is deployed alongside the backend by the same
`docker-compose.yml` / `terraform/main.tf`, served at
**https://finreport.lab.anydef.de** (`192.168.100.50:3000` on `services-lan`,
OPNsense/HAProxy + unbound, same convention as `finreport-be.lab.anydef.de`).
`finreport-fe/Dockerfile` builds it; `GRAPHQL_URL` is read via
`$env/dynamic/private` (`graphqlBackend.ts`) so the compose service points it
at `http://192.168.100.45:8080/graphql` **at runtime**, not at build time —
dev profiles above are unaffected. CI builds/pushes it with `just build-fe`
(`.gitea/workflows/build-deploy.yaml`), a sibling to the backend's `just
build`.

## Multi-agent development

This repo is being prepped to support multiple Claude Code agents working in parallel. See `docs/multi-agent-setup.md` for the running log of what's been set up and why.

Natural parallelization seams:
- `finreport-rs` (backend/Rust) vs. `finreport-fe` (frontend/SvelteKit) — separate toolchains, separate CLAUDE.md context.
- Within `finreport-rs`, individual crates (`comdirect-rs`, `categorizer`, `entity`/`migration`) are reasonably independent, but `webapp` depends on all of them and the GraphQL schema is a shared contract — two agents changing it concurrently will conflict.

Use `git worktree add ../finreport-worktrees/<branch-name> -b <branch-name>` to give a parallel agent its own working copy (worktrees are kept as sibling dirs, not nested — see `docs/multi-agent-setup.md`).

### Shared `CARGO_TARGET_DIR`

The `justfile` exports `CARGO_TARGET_DIR` for every recipe it runs, pointed at
`finreport-worktrees/.cargo-target` — a single directory shared by every
worktree *and* the main checkout, computed from the sibling-directory layout
above rather than hardcoded (so it resolves the same way whether `just` runs
from `finreport/` or any `finreport-worktrees/<branch>/`). N parallel agents
then share one dependency build instead of each paying a cold compile; Cargo
locks the directory itself, so concurrent builds serialize rather than
corrupt one another. A `CARGO_TARGET_DIR` already set in the calling shell
takes precedence. `cargo` invocations outside `just` (an editor's rust-analyzer,
a manual `cargo build`) don't pick this up automatically — export it yourself
in that shell if you want the same sharing there.

### Agent working rules

Binding for every agent working in this repo:

- **Never push.** No `git push`, to any remote, ever — that is the one hard git
  limit. Everything local is allowed.
- **Finish by merging into `main`.** Work goes on a feature branch (in a
  worktree, `../finreport-worktrees/<branch>`, when agents run in parallel),
  then gets merged back into local `main` with `git merge --no-ff`, matching
  this repo's existing merge-commit history. No *direct* commits on `main` —
  but the merge commit itself is expected, and work left sitting on a branch
  is not finished.
- **Never scan the home (`~`) or root (`/`) folder.** Stay inside the repo and its worktrees.
- **Model roles:**
  - Opus 5 writes specs.
  - Sonnet 5 writes code.
  - Haiku runs simple scripts, tests and lints.
  - Opus 5.5 evaluates/validates results and orchestrates.
- These roles are the default. The orchestrator may pick a different model, subagent or effort level per task. Keep costs low without lowering quality.
- Independent work may run in parallel across agents.
- **Cost discipline:** delegate mechanical work — test and lint runs, doc and
  prose edits, regenerating generated files, grep-style spot checks — to a
  cheap Haiku subagent rather than running it on the orchestrating model. Check
  delegated *numbers* instead of trusting the summary: a Haiku sweep in this
  repo once reported 270 passing tests where the real total was 290. There is
  no AI-credit (AIC) budget in this environment — that was a Copilot-era
  concept, and the old `assistant_usage_events` spend query does not apply.
- **Code quality:** code must be readable, well modularized and well tested.
- **Local runnability:** everything must be runnable locally. Docker is fine, e.g. Postgres via `just db-up` or a local Kafka broker for dev.
- **Never start a server.** No dev server, preview server or other
  long-running process on the user's behalf — not on a non-default port, and
  not to check your own work. Print the command instead (`just dev-fe-mocks`,
  then the URL) and let them run it. Verify with tests and `npm run check`; if
  something genuinely needs a browser, say so in your report.
- **One mergeable slice per agent.** Split a feature on seams that can be
  verified and merged independently — storage (topic, migration, entity,
  projection) / the read side (resolver, SQL, tests) / UI wiring — rather than
  one end-to-end brief. A brief covering all three ran over six minutes with
  nothing reviewable. If a slice cannot be cut below roughly 15 minutes of
  agent work, tell the user the expected wait before launching it.
- **Use the `just` recipes.** `just dev-fe-mocks` is the documented way to
  review UI on mocks (it also runs `npm ci` when `node_modules` is stale); a
  raw `npx vite dev` skips that. Same for `just test` / `just lint`.
- **Test baselines**, so a delegated run can be checked rather than trusted:
  **685** Rust unit tests via `just test`, **779** integration via
  `just test-integration`, **773** frontend (38 files) via
  `npm run test:unit` (all re-measured 2026-10-27). **These go stale within a
  session** - five earlier pairs did, and each one cost an agent a paragraph
  explaining the mismatch, so refresh this line in the same commit that adds
  tests, and treat a small discrepancy as a stale baseline rather than as
  something you broke.
  Report the verbatim `test result:` lines and show the addition. A
  truncated `cargo test` run stops at the first failing crate and
  undercounts badly — pass `--no-fail-fast` when totalling.
- **A single red run is not yet evidence.** The shared `CARGO_TARGET_DIR`
  means a concurrent build in another worktree can clobber artifacts and
  produce bogus "unresolved import"/"Unknown field" errors, or a spurious test
  failure. `touch finreport-rs/entity/src/lib.rs finreport-rs/webapp/src/lib.rs`
  and retry before believing it.
- **Integration tests need `finreport-wp4-pg`** on `127.0.0.1:55435`
  (`webapp/tests/common/mod.rs` expects exactly that container). If every test
  fails at `common/mod.rs` with `PoolTimedOut`, the container is down —
  `docker start finreport-wp4-pg` — it is not your change. Use your own
  database on it, or your own container under a distinct name, and clean up.
- **Resolving a merge by unioning both sides is a trap.** Two branches adding
  a topic, a migration and a dispatch arm in the same places look additive,
  but a conflict that splits mid-function leaves the closing brace attached to
  the wrong block. Build after every such resolution, and bump any
  `[&str; N]` array length the union changed.
- **Judgement calls:** agents may make their own assumptions and decisions, unless they are security-critical or harmful. Those go to the user.
