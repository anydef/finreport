# Kafka (Redpanda) migration — phase 1: dual-write

Design doc for adding a Redpanda broker (Kafka API) alongside the importer's
existing Postgres writes. This is **not** a cutover: Postgres stays the
source of truth, the GraphQL backend keeps reading from Postgres, and nothing
here changes what `import_transactions` writes to `account` /
`account_balance` / `account_transactions` today. The importer additionally
publishes an event per record it already writes. If this phase goes wrong,
the fix is "stop publishing" — Postgres is unaffected either way.

Status: implemented (phase 1). The importer, topics, and Terraform module
described below exist in the codebase. The broker itself is **central
homelab infrastructure at `kafka.lab.anydef.de:9092`, not deployed by this
repo** — this repo owns only its own topics on it (`terraform/kafka`).
Assumptions that were unverified at design time and remain so are still
called out explicitly rather than stated as fact.

## 1. Why

The importer (`finreport-rs/webapp/src/bin/import_transactions.rs`) currently
does one thing with each Comdirect response: parse it into an ORM model and
upsert it into Postgres. That has two effects worth separating:

- **The bank's answer at a point in time is thrown away once it's folded
  into the current-state tables.** `account_transactions` keeps whatever
  fields the entity happened to model (see the `ActiveModel` construction in
  `import_transactions.rs::run_import`); anything Comdirect sent that isn't
  mapped to a column is gone the moment the insert succeeds. If a future
  feature needs a field nobody thought to store, there's no way to get it
  back short of re-importing from the bank (and the transactions endpoint,
  see §4, can't even be asked for a specific date range).
- **Postgres is both the transactional log and the read model**, so anything
  that wants a feed of "what happened" (a future audit trail, an alternate
  read model, a categorizer that reacts to new transactions instead of being
  invoked as a separate batch job) has to poll Postgres and diff, because
  there is no append-only record of arrivals independent of the upsert
  target.

An event log fixes both: the raw bank payload is captured once, verbatim, at
import time, independent of whatever the relational schema looks like today
or how it changes later. Any number of downstream consumers can replay it
without touching the importer or the bank.

This phase adds that log **without removing or reshaping anything that
exists**. The importer dual-writes: Postgres inserts stay byte-for-byte as
they are today, and a Kafka publish is added alongside each one. A later,
separate migration is what would flip a consumer to read from Kafka instead
of Postgres, retire the dual-write, or make the log authoritative — none of
that is decided here (see §6).

## 2. Topic design

One broker (Redpanda, Kafka-API-compatible), assumed single-node for now, so
every topic is replication factor 1. All are 1 partition — the total event
volume here is one household's transactions, not a scale problem, and 1
partition keeps per-account ordering trivial (see key below) without needing
a partitioner.

| Topic | Key | Cleanup policy | Retention | Message value |
|---|---|---|---|---|
| `finreport.account` | `account_id` | `compact` | n/a (compaction keeps latest per key) | Raw Comdirect account JSON (the `account` object nested in an `AccountBalance`, see `balance_model.rs`) |
| `finreport.account-balance` | `account_id` | `delete` | forever (no retention limit set) | Raw Comdirect balance JSON — the full `AccountBalance` entry as returned by `/banking/clients/user/v2/accounts/balances`, one message per import cycle per account |
| `finreport.transaction` | `account_id` | `delete` | forever | Raw Comdirect transaction JSON — one message per `Transaction` entry from `/banking/v1/accounts/{uuid}/transactions`, byte-for-byte as received |
| `finreport.import-watermark` | `account_id` | `compact` | n/a | Our own small JSON — see §3 |

All four topics are created by Terraform using the `Mongey/kafka` provider,
as the `terraform/kafka` child module of this repo's existing root module —
same state, same `just deploy`. This repo does not provision the broker
itself; `kafka.lab.anydef.de` is expected to already exist and be reachable
whenever this module plans or applies. (An earlier iteration of this design
ran the broker as part of finreport's own stack and had to sequence around
bringing it up first; centralizing the broker removed that problem instead
of solving it in-repo.)

### Raw-payload rule

For the first three topics, the message **value is the exact bytes Comdirect
returned**, not a re-serialization of whatever `serde` struct the Rust code
deserializes into. Concretely: the importer already does
`response.json::<T>()` today (see `account_client.rs`); this phase adds
capturing `response.bytes()` (or `response.text()`) *before* or *alongside*
that deserialization step, and publishes those bytes unmodified.

Why this matters: `Transaction`, `Account`, and `AccountBalance` (in
`comdirect-rs/src/comdirect/transaction.rs` and `balance_model.rs`) are
partial views of what Comdirect sends — several fields are commented out
(`currency`, `availableCashAmount`) or default-on-absence
(`Creditor::iban`/`bic`, `Remitter::holder_name`, `remittance_info`) because
the existing importer only needs a subset and tolerates the rest being
missing. If the Kafka payload were "serialize the Rust struct back to JSON,"
every field the struct doesn't model would be silently and permanently lost
a second time, defeating the entire point of capturing the log independent
of the current relational schema (§1). Re-serializing also couples the topic
schema to whatever `#[serde(rename = ...)]` mapping happens to exist in
`comdirect-rs` today, which is exactly the coupling this log is meant to
avoid.

Importer-side metadata — which Comdirect login (config key, e.g. the
`profile.key` from `ComdirectProfile`) produced the record, and the
wall-clock import time — goes in **Kafka headers**, not the payload. This
keeps the payload provably identical to what the bank returned (useful for
later debugging: "did Comdirect actually send this, or did we mangle it"),
and keeps the two concerns — bank data vs. our own bookkeeping — separable
without a wrapper envelope. A consumer that only cares about bank data reads
the value and ignores headers; a consumer building operational tooling
(e.g. "show me every event produced by login X") reads headers without
touching the payload.

The watermark topic is the deliberate exception: its value is our own
small JSON, not a Comdirect response (there's nothing from the bank to
capture — see §3).

## 3. Resume points

On startup, `run_account` in `import_transactions.rs` bootstraps a session
and then runs `run_import` on a schedule (`IMPORT_INTERVAL`, currently 4h).
Once dual-write exists, each import cycle should only need to *publish*
records the log doesn't already have — otherwise every restart re-publishes
the account's entire transaction history to `finreport.transaction`. That
needs a per-account "resume point": the newest transaction reference (or
booking date) already published, checked at the start of each cycle so
publishing can stop early once it's reached (see §4 for why this has to be
early-stop rather than a query parameter).

**Chosen: a compacted `finreport.import-watermark` topic**, one message per
account, keyed by `account_id`, value something like:

```json
{ "account_id": "...", "last_reference": "...", "last_booking_date": "...", "updated_at": "..." }
```

The importer writes an updated watermark record after each successful
publish pass for an account, and reads it back (by consuming the compacted
topic to the end, or via an external table / KTable-style read if the
client library supports it) at the start of the next cycle or on process
restart.

**Alternatives considered:**

| Approach | Pros | Cons |
|---|---|---|
| **Compacted watermark topic (chosen)** | Reading the current watermark for all accounts is O(accounts) — read the compacted topic to the end, one record per account, regardless of how much transaction history exists. Independent of `finreport.transaction`'s retention: even if that topic's retention were ever shortened, the watermark still exists as long as compaction has run. | A second topic to create and administer. Costs one extra write per import cycle per account. Compaction is a background broker process, not instant — the topic can briefly hold more than one record per key between compaction runs (fine for a "read to end, keep latest" reader, but worth knowing). |
| **Tail-scan `finreport.transaction` backwards on startup** | No extra topic. | Cost is O(transactions since the account's last watermark), not O(accounts) — a busy account after a long gap means scanning a lot of records just to find where to resume. Breaks entirely if `finreport.transaction` ever gets a retention limit and the watermark's position has aged out from under it. A quiet account (few transactions) can require scanning arbitrarily far back to find *any* prior record for it, since the topic is shared across accounts. |
| **Full replay of `finreport.transaction` on startup, dedupe downstream** | Simplest possible logic — no resume-point bookkeeping at all, consumers just dedupe on `reference`. Exact by construction. | Startup cost grows without bound as history accumulates; this is explicitly the failure mode retention-forever on that topic is choosing to accept for the topic's *content*, and re-adding it at the *consumer* level too compounds the problem rather than avoiding it. |
| **Derive the watermark from Postgres** (e.g. `MAX(booking_date)` per account from `account_transactions`) | Trivially available today — the data already exists, no new topic, no new write. | Couples the new Kafka path to Postgres, which is precisely the dependency this migration exists to move away from (§1). Whatever reads the watermark this way has to be rewritten or removed at cutover, which just relocates the problem rather than solving it, and it doesn't work at all once Postgres is no longer authoritative. |

The compacted topic won because it's the only option whose cost doesn't grow
with either transaction volume or the transaction topic's retention policy,
and it doesn't tie the new path back to Postgres. The price — one more topic
and one more write per cycle — is small and fixed.

## 4. The pagination constraint (riskiest assumption)

**The Comdirect bank-account transactions endpoint has no date filter.**
`AccountClient::get_account_transactions` (`comdirect-rs/src/comdirect/account_client.rs`)
calls:

```
GET /banking/v1/accounts/{account_id}/transactions?transactionState=BOOKED&paging-first={index}
```

The only query parameters this endpoint accepts are `transactionState` and
`with-attr`; there is no `min-bookingDate` / `max-bookingDate` equivalent.
(The Comdirect *depot* transactions endpoint does have `max-bookingDate` —
it's a different endpoint for securities, not bank accounts, and the
importer doesn't use it.) This means "only import what's newer than the
watermark" **cannot be pushed down to the API as a request parameter** — the
importer has to fetch pages and decide client-side where to stop.

**The approach: early-stop pagination from the newest page.** Given a
watermark (last-seen reference / booking date), page through
`get_account_transactions` starting from the first page and stop as soon as
a record at or older than the watermark is seen, instead of always walking
every page as `get_accounts` → `get_account_transactions` does today (see
the `for index in (page_size..total_transactions).step_by(page_size)` loop
in `comdirect-rs/src/comdirect/accounts.rs`, which currently always fetches
every page).

**This depends entirely on the API returning newest-first — which is not
documented anywhere in the Comdirect API and has not been verified against
production responses.** The current importer code doesn't rely on ordering
at all (it just fetches every page every time and upserts by `reference`,
so order doesn't matter to it). Early-stop pagination is new behavior that
*introduces* an ordering dependency that didn't exist before. If the
assumption is wrong — if Comdirect returns oldest-first, or an order that
isn't stable across pages, or an order that isn't stable over time for the
same account — early-stop will quietly stop too early and silently miss
transactions, with no error to point at.

**Runtime guard (required, not optional):** the importer must not trust this
assumption blindly. At minimum, on every page fetched, assert that booking
dates are monotonically non-increasing within the page and across page
boundaries; if that invariant is violated, abort early-stop for that account
for that cycle and fall back to a full walk (i.e., behave like today's
importer does unconditionally) rather than risk under-importing. This is
flagged as **the single riskiest assumption in this whole design** — get it
wrong and the failure mode is silent data loss, not a crash.

## 5. Operating it

Broker: `kafka.lab.anydef.de:9092`, plaintext, no TLS or SASL — a central
homelab service, not something this repo deploys or configures. See whatever
repo/runbook owns that host for broker-level operations (upgrades, node
health, disk); this section only covers finreport's own topics on it.

Using [`rpk`](https://docs.redpanda.com/current/reference/rpk/) against that
broker:

```bash
# list topics
rpk topic list -X brokers=kafka.lab.anydef.de:9092

# describe a topic's config (partitions, cleanup.policy, retention)
rpk topic describe finreport.transaction -X brokers=kafka.lab.anydef.de:9092

# tail new events for an account (client-side filter — the topic isn't partitioned by account)
rpk topic consume finreport.transaction -X brokers=kafka.lab.anydef.de:9092 | jq 'select(.key == "<account_id>")'

# check a single account's current watermark: compacted topic, so consuming
# from the start and keeping the last record per key gives the current value
rpk topic consume finreport.import-watermark -X brokers=kafka.lab.anydef.de:9092 \
  --offset start -f '%v\n' | jq -c 'select(.account_id == "<account_id>")' | tail -n 1
```

**Forcing a re-import for one account:** publish a watermark record for that
`account_id` with an older `last_booking_date` / no `last_reference` (or
produce a tombstone — a compacted-topic record with a null value — if the
watermark reader treats "no record" as "import everything"):

```bash
# reset by writing an old/empty watermark
echo '{"account_id":"<account_id>","last_reference":null,"last_booking_date":null,"updated_at":"<now>"}' \
  | rpk topic produce finreport.import-watermark -X brokers=kafka.lab.anydef.de:9092 --key '<account_id>'

# or delete the key outright via a tombstone (null value), if supported by however produce is invoked
```

The importer reads each account's watermark **once, at process startup**
(`load_watermarks` in `webapp::kafka::watermark`), not on every import cycle
— so a reset written while an account's task is already running won't take
effect until that process restarts. Once it does, early-stop pagination
simply won't find anything to stop at, so it walks further back — bounded by
whatever the transaction topic's own history covers plus what Comdirect's
API still returns for that account.

## 6. Open questions / not yet decided

- **What happens at cutover.** This doc only covers dual-write. Whether a
  later phase makes Kafka (or a consumer fed by it) the system of record,
  what triggers the cutover, whether Postgres writes are ever actually
  removed, and how a consumer migrates its read path — none of that is
  designed here.
- **Joint accounts visible from two Comdirect logins.** `account` has a
  `UNIQUE` constraint on `iban` (see `m20220101_000001_account.rs`,
  `string_uniq(Account::IBAN)`). Root CLAUDE.md already documents the
  Postgres-side consequence: if the same IBAN arrives under two different
  `accountId`s (two logins seeing the same joint account), the second
  insert trips the unique index, logged as an error, and its balances and
  transactions fail their foreign keys. The Kafka topics as designed here
  key everything by `account_id`, not `iban`, so **the same joint account
  would land as two independent, undeduplicated key-spaces in
  `finreport.account`, `finreport.account-balance`, and
  `finreport.transaction`** — the event log would not inherit Postgres's
  (partial, error-logging) protection against this, and nothing currently
  proposed reconciles the two `accountId`s back to one IBAN. Whether that
  needs deduplication at publish time, at consume time, or not at all
  (if no joint account currently exists across the configured logins) is
  unresolved.
- **Delivery guarantees.** During dual-write, the Kafka producer is
  best-effort: Postgres remains authoritative, and a failed or dropped
  publish is not treated as an import failure (the existing Postgres
  error-and-continue pattern in `run_import` — e.g. balance/transaction
  insert failures are logged and looped past, not propagated — is the
  model to follow for publish failures too, though this hasn't been
  implemented). This means the event log is not guaranteed complete
  relative to Postgres during this phase; exactly-once or at-least-once
  producer semantics, retry-on-publish-failure, and whether a failed
  publish should be retried before or after the next Postgres write in the
  same cycle are all undecided.
- **When the watermark is read**: only at process startup, or re-read at
  the top of every import cycle (relevant if something external — e.g. a
  manual reset per §5 — should take effect without restarting the
  importer). Not decided.
- **Broker durability/replication** beyond replication factor 1 on a single
  node — acceptable for a design/dual-write phase, but not evaluated for
  whatever "phase 2" looks like.
