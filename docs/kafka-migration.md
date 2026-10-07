# Kafka (Redpanda) migration — phase 2: cutover

Design doc for the importer's move from dual-write to **Kafka-only**. Phase 1
(see git history for the prior version of this file) added a Redpanda publish
alongside every Postgres write, with Postgres remaining authoritative and the
publish best-effort. Phase 2 removes the Postgres half entirely from the
importer: `import_transactions` no longer opens a database connection at
all, `APP_kafka_brokers` goes from optional to a hard startup requirement,
and the four topics (§2.1 of `docs/specs/iteration-1.md`) become the system
of record for raw bank data. A separate process, the projector (§2.3,
WP3-owned), rebuilds the relational read model from the topics; nothing in
this repo reads directly from the importer's writes into Postgres anymore,
because there are none.

Status: implemented (phase 2, WP1). The broker itself remains **central
homelab infrastructure at `kafka.lab.anydef.de:9092`, not deployed by this
repo** — this repo owns only its own topics on it (`terraform/kafka`).

## 1. Why

Phase 1's dual-write existed to capture the event log without disturbing the
working Postgres-backed importer while the design settled. That purpose is
now served: the envelope contract (`webapp/src/kafka/envelope.rs`, frozen by
WP0, §2.2) is stable, the projector consumes it, and continuing to also
write Postgres directly from the importer would mean two independent paths
that have to agree on every record forever. Phase 2 collapses that to one:

- **The importer's only job is capture-and-publish.** It fetches from
  Comdirect, builds the envelope headers, and publishes. It does not decide
  what the relational schema looks like, does not upsert anything, and does
  not need `APP_database_url` to run (see `webapp/src/bin/import_transactions.rs`
  — grep it for `sea_orm`/`DbConn`/`Database::connect` and there is nothing to
  find).
- **Postgres is now a derived read model, rebuilt by the projector from the
  topics.** If the mapping from envelope to row ever needs to change —
  a new column, a bug fix in normalization (§2.5) — the fix is in the
  projector, and replaying the topics from the start reproduces the new
  shape. This is the entire point of capturing raw, unmodified bank bytes in
  phase 1: phase 2 is where that investment starts paying off.
- **Publish failure is now data loss, not a degraded side channel.** In
  phase 1 a dropped publish just meant the event log briefly lagged Postgres,
  which still had the authoritative write. There is no such fallback now:
  a transaction that fails to publish is a transaction nobody will ever see
  again unless the next import cycle's early-stop pagination (§4) happens to
  re-fetch it. The importer therefore treats a transaction publish failure as
  `error`-level and **does not advance that account's watermark for the
  cycle** — see §3.

## 2. Topic design (unchanged)

Same four topics as phase 1, same keys, same partitioning (1 partition,
replication factor 1 — see `terraform/kafka/main.tf`, which phase 2 does not
touch: legacy-backfill (§6) publishes into the existing topics under live
keys, it does not add any):

| Topic | Key | Cleanup policy | Retention | Message value |
|---|---|---|---|---|
| `finreport.account` | `account_id` | `compact` | n/a (compaction keeps latest per key) | Account JSON — raw Comdirect bytes for live imports, our own reconstruction for legacy-backfill (§6) |
| `finreport.account-balance` | `account_id` | `delete` | forever (no retention limit set) | Balance JSON, one message per import cycle per account (or per legacy `(account_id, date)` row, §6) |
| `finreport.transaction` | `account_id` | `delete` | forever | Transaction JSON, one message per transaction |
| `finreport.import-watermark` | `account_id` | `compact` | n/a | The importer's own small JSON resume-point record (§3) — unchanged, and still importer-private: `fixture-replay` deliberately does not touch this topic, and the projector never consumes it (§2.3) |

### Raw-payload rule, revised for phase 2

Phase 1 stated this rule as absolute: "the value is the exact bytes Comdirect
returned, always." That is no longer quite true, and the rule now has an
escape hatch that every consumer must check rather than assume:

**The payload is the exact bytes Comdirect returned *when the `origin`
header is `source`*.** `is_bank_verbatim(&envelope)` (`kafka/envelope.rs`) is
the one function that answers this; nothing should re-derive the check
independently. When `origin` is `legacy-backfill` instead (§6), the payload
is **our own reconstructed JSON**, built from columns in the renamed
`legacy_*` Postgres tables, not bank bytes at all — those tables predate the
envelope contract and never captured a raw response, so there is nothing
verbatim to replay. A consumer that cares whether a payload is trustworthy
provenance (bit-for-bit what the bank said) rather than a best-effort
reconstruction from old relational data must check `origin`/
`is_bank_verbatim`, not assume every record in these topics is bank-sourced
just because most of them still are.

Why keep publishing legacy data into the *same* topics instead of a separate
one: downstream consumers (the projector chief among them) want one
chronological, per-key stream per entity type, not "the live stream, plus a
second stream for history, merge them yourselves." The header, not the
topic, is what carries the provenance distinction.

Everything else about the raw-payload rule is unchanged from phase 1: the
importer captures response bytes before deserializing, and the Kafka value
is those bytes, not a re-serialization of whatever subset of fields the
`comdirect-rs` structs happen to model.

### Header contract, now frozen (§2.2)

Phase 1 carried two headers (`account_key`, `imported_at`, informally).
Phase 2's envelope (`webapp/src/kafka/envelope.rs`, WP0-owned, frozen) is the
full contract every publisher and consumer agrees to:

| Header | Required | Notes |
|---|---|---|
| `source` | yes | Stable source id, lowercase. Only `comdirect` exists today. |
| `source_account_id` | yes, on every topic including `account` | The source's own account id. Sent even on the `account` topic, where the payload already names it — WP1 chose uniformity (every record on every topic carries it) over the strict minimality the written spec table literally implies, because the committed fixture corpus (`webapp/fixtures/manifest.json`) carries it on `account` records too; treat the fixtures as the tie-breaker if this ever looks contradictory again. |
| `origin` | yes | `source` (fetched live) or `legacy-backfill` (§6). |
| `schema_version` | yes | Header-contract version, currently `1`. Bumped only if headers themselves change shape. |
| `imported_at` | yes | RFC 3339. Wall-clock fetch time for live records; a deterministic placeholder or the legacy row's own date for backfilled ones (§6). |
| `comdirect_account_key` | yes | Config key of the importing login (`0`, `1`, ... or `default`). A sentinel (`legacy-backfill-reconstruction`) for legacy rows, which were never tied to a specific login. |
| `comdirect_account_name` | no | That login's human-readable label, when configured. |

`Envelope::parse` (the read side) applies phase-1-compatible defaults for
anything absent (`source` → `comdirect`, `origin` → `source`,
`schema_version` → `1`, `imported_at` → the Kafka message timestamp) so that
the handful of deliberately-headerless phase-1-shaped fixtures in the
corpus (`ACC1-PHASE1-0001`) still parse into a valid envelope rather than
being rejected outright — the projector logs and continues past a record
missing `source_account_id` (`ACC1-MISSING-ACCOUNT-0001`) rather than
crashing the whole topic.

## 3. Resume points (mechanism unchanged, stakes raised)

Same mechanism as phase 1: a compacted `finreport.import-watermark` topic,
one record per account, read at process startup
(`load_watermarks` in `webapp::kafka::watermark`) and written after each
import cycle. What changed is what a failure to publish now means:

- **Account/balance snapshots** publish best-effort
  (`producer::publish_best_effort`) — a dropped publish just means that
  cycle's snapshot is missing; the next cycle re-fetches the current state
  anyway, so nothing is permanently lost.
- **Transactions do not.** `producer::publish` returns a `Result`, and
  `run_import` in `import_transactions.rs` tracks whether every transaction
  publish in a cycle succeeded. If any failed, the account's watermark is
  **not** advanced for that cycle — advancing it past an unpublished
  transaction would mean the next cycle's early-stop pagination (§4) never
  revisits it, silently losing it forever. The failure is logged at `error`
  and the import loop stays alive; the next scheduled cycle simply retries
  from the same (unmoved) watermark.

## 4. The pagination constraint (unchanged, still the riskiest assumption)

Unchanged from phase 1 and still load-bearing: the Comdirect bank-account
transactions endpoint has no date filter, so "only import what's newer than
the watermark" is early-stop client-side pagination, not a request
parameter. See `comdirect-rs/src/comdirect/transaction.rs`
(`page_is_sorted_newest_first`, `filter_page_for_stop`, `PageOutcome`,
`ImportStop`) for the runtime guard: if a page's booking dates are not
monotonically non-increasing, early-stop aborts for that account that cycle
and falls back to a full walk rather than risk silently under-importing.
**This code is unchanged by the phase-2 cutover and must stay that way** —
nothing about moving from Postgres to Kafka writes changes the ordering
assumption or how the guard has to behave; it would be a mistake to touch it
as part of this migration.

## 5. Operating it

Unchanged from phase 1 for the live broker
(`kafka.lab.anydef.de:9092`, plaintext, central homelab infrastructure this
repo does not deploy):

```bash
# list topics
rpk topic list -X brokers=kafka.lab.anydef.de:9092

# tail new events for an account (client-side filter — not partitioned by account)
rpk topic consume finreport.transaction -X brokers=kafka.lab.anydef.de:9092 | jq 'select(.key == "<account_id>")'

# check a single account's current watermark
rpk topic consume finreport.import-watermark -X brokers=kafka.lab.anydef.de:9092 \
  --offset start -f '%v\n' | jq -c 'select(.account_id == "<account_id>")' | tail -n 1
```

**Local dev / verification, without the central broker**: run your own
throwaway Redpanda container (see `just redpanda-console` for pointing the
console UI at one, or any single-node Redpanda/Kafka-compatible image) and
point the tools at it with `APP_kafka_brokers=<host>:<port>`:

```bash
# replay the whole WP0 fixture corpus into a fresh broker — fills
# finreport.account, finreport.account-balance and finreport.transaction
# (never the watermark topic; see fixture-replay below)
APP_kafka_brokers=127.0.0.1:19192 cargo run --bin fixture-replay -- webapp/fixtures

# one-off republish of the legacy_* tables into the same topics,
# origin=legacy-backfill, safe to re-run (§6)
APP_kafka_brokers=127.0.0.1:19192 APP_database_url=postgres://... \
  cargo run --bin legacy-backfill
```

**Forcing a re-import for one account**: unchanged from phase 1 — publish a
watermark record for that `account_id` with an older/empty
`last_reference`/`last_booking_date`, or a tombstone, to
`finreport.import-watermark`. The importer only reads watermarks at process
startup, so a reset takes effect on the next restart, not mid-run.

**Tombstoning every watermark** (forcing a full re-import across all
accounts, e.g. after a legacy backfill) is `legacy_backfill --tombstone-watermarks
[--dry-run]` (same binary, a separate mode from the default backfill run):
it drains `finreport.import-watermark` with the same
`watermark::load_watermarks` read the importer itself uses, collects every
live key, and — unless `--dry-run`, which only lists the keys it would
tombstone — publishes a null-value record for each. The key-collection step
(`live_watermark_keys`) is plain, network-free and unit-tested; only the
publish step touches Kafka.

## 6. Legacy backfill (§2.8)

The `legacy_backfill` binary (`webapp/src/bin/legacy_backfill.rs`) is a
one-off, idempotent tool: it reads the renamed `legacy_account`,
`legacy_account_balance`, and `legacy_account_transactions` Postgres tables
(data imported before the event log existed, with no corresponding Kafka
history) and republishes it into the regular live topics — not a separate
"legacy" topic — tagged `origin=legacy-backfill` so it's distinguishable
from live-sourced data without needing a different stream (§2, "raw-payload
rule, revised for phase 2").

**Idempotency** is the property that makes repeated runs (after a partial
failure, or just "did I actually run this yet") safe: before publishing
anything, the tool scans each of the three live topics to the end (mirroring
how `watermark::load_watermarks` drains the compacted watermark topic — a
manual-partition-assign, no-consumer-group read) and only publishes identities
not already present:

- **`account` and `transaction`** (compacted / append-only-by-key in
  practice): simple key presence is enough — if any record for that key
  exists, skip it. A tombstone (null value) for a key is treated as "not
  present", so a deliberately deleted key would be re-backfillable, which is
  the conservative choice between "never re-publish a deleted key" and
  "don't let a tombstone silently and permanently swallow legacy data."
- **`account-balance`** needs a richer identity than key-presence, because
  the topic is `cleanup.policy=delete, retention.ms=-1` — every historical
  observation is meant to persist, not just the latest one per account. Key
  presence alone would see *any* balance record for an account (e.g. from a
  live import) and wrongly skip backfilling that account's entire legacy
  balance history. The tool dedups on `(account_id, date)`, deriving the date
  the same way the projector dates each origin (§2.5):
  `legacy-backfill`'s own `ReconstructedBalance` payload carries its own
  `date` field — the tool reads that first, matching the projector's
  `LegacyMapper::map_balance` (which reads `balance.date`, never
  `imported_at`) — falling back to the `imported_at` header only for records
  whose payload has no `date` field at all (the live Comdirect `{value,
  unit}` shape, which `ComdirectMapper::map_balance` dates from
  `imported_at` for the same reason). Reading `imported_at` unconditionally
  would misdate a re-scan against the backfill's own prior output: a
  `legacy-backfill` record's `imported_at` is the fixed import-time
  placeholder, not the balance's own observed date, so the two origins must
  be told apart rather than treated as one rule.

**Sentinel values**, used because legacy rows lack fields the envelope
otherwise requires: `comdirect_account_key` is set to the fixed string
`legacy-backfill-reconstruction` (no real login key exists for a
pre-event-log row — nothing ties it to a specific Comdirect login), and
`legacy_account` rows (which have no timestamp column at all) get a fixed
`imported_at` placeholder of `1970-01-01T00:00:00Z`. Both are deterministic
precisely so that a re-run reconstructs byte-identical payloads and headers
— required for the key/identity-based skip logic above to actually recognize
"already published" on the next run, and required for re-running the tool to
be a no-op rather than silently publishing slightly different headers each
time.

**Reconstructed payload shape**: there is no bank response to replay for
legacy rows, so the tool serializes its own JSON from the legacy row's
columns (snake_case, matching the committed
`fixtures/payloads/legacy/transaction-0001.json` reference shape exactly —
asserted by a unit test). This is a WP1 decision, not something the spec
pins down beyond that one transaction fixture; the same column-to-JSON
convention was extended to `legacy_account` and `legacy_account_balance`
rows since no other precedent exists for those. Anything parsing
`origin=legacy-backfill` records downstream (the projector's `LegacyMapper`,
§2.4, WP3-owned) needs to agree on this shape — it is effectively an
implicit cross-WP contract riding on this doc and the fixture file, not on
`envelope.rs` itself.

## 7. fixture-replay (§7)

`fixture_replay` (`webapp/src/bin/fixture_replay.rs`) publishes the
committed WP0 fixture corpus (`webapp/fixtures/manifest.json` +
`payloads/**`) into a broker, verbatim: for each manifest entry it reads the
referenced payload file, builds Kafka headers from *exactly* the key/value
pairs present in that entry's `headers` object — no inference, no
defaulting — and publishes to the entry's `topic`/`key`. This is what makes
the corpus useful for exercising edge cases no real import would produce on
demand: the manifest includes a deliberately headerless record
(`ACC1-PHASE1-0001`, exercising `Envelope::parse`'s phase-1-compatible
defaults), a record missing `source_account_id`
(`ACC1-MISSING-ACCOUNT-0001`, exercising the projector's skip-and-log path),
and a `legacy-backfill`-origin record (`LEGACY-0001`) — none of which a
running importer or `legacy-backfill` would ever construct on their own,
since both always emit a complete, valid envelope.

It never touches `finreport.import-watermark`: that topic is importer-private
bookkeeping, not part of the entity data the corpus is modeling, and nothing
in the manifest targets it.

## 8. Open questions / not yet decided

Carried over from phase 1, still unresolved and not in scope for this
cutover:

- **Joint accounts visible from two Comdirect logins** still key everything
  by `account_id`, not `iban`; the same joint account under two different
  `accountId`s still lands as two independent, undeduplicated key-spaces.
  Unchanged by this phase.
- **Broker durability/replication** beyond replication factor 1 on a single
  node — unevaluated, same as phase 1.

Resolved by phase 2 (listed here only so a reader of the phase-1 doc's
history doesn't go looking for answers already settled):

- ~~What happens at cutover~~ — this is the cutover: Postgres is no longer
  written by the importer, and the projector is the only path from topics to
  relational rows.
- ~~Delivery guarantees~~ — account/balance snapshots stay best-effort;
  transaction publishes are not, and a failure blocks that cycle's watermark
  advance (§3).
- ~~When the watermark is read~~ — still only at process startup; not
  revisited by this phase.
