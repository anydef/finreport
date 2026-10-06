# Fixture corpus (WP0, §9)

Realistic comdirect-shaped payloads for `fixture-replay` (WP1) to publish onto
the ingest topics, and the basis the frontend's GraphQL mocks
(`finreport-fe/src/lib/graphql/mocks/`) are derived from.

## Layout

- `payloads/accounts/<key>.json` — raw comdirect `account` sub-objects
  (`comdirect_rs::balance_model::Account` shape), one per account.
- `payloads/balances/<key>-<date>.json` — raw comdirect `balance` sub-objects
  (`{value, unit}`), three observations per account across the six-month
  window (2024-01 .. 2024-06).
- `payloads/transactions/<key>/<reference>.json` — raw comdirect transaction
  objects (`comdirect_rs::transaction::Transaction` shape), spanning the same
  six months.
- `payloads/legacy/transaction-0001.json` — a **reconstructed** transaction in
  the shape of the renamed `legacy_account_transactions` columns (§2.8): this
  is our own JSON, not a bank payload, because there is no raw payload to
  forward for pre-dual-write history.
- `manifest.json` — the ordered list of Kafka records `fixture-replay`
  publishes: `{ topic, key, payload, headers, note }`. `headers` only lists
  headers that are actually set on that record — a key absent from `headers`
  is a header that record never carries, exactly like a real headerless
  phase-1 record (§2.2's defaults apply on the read side, not here).

## Two accounts, deliberately opposite cash flow

- `acc-1` ("Main"): income (`ACME GmbH` salary) consistently exceeds spending
  — ends the period in `CashflowNodeKind.NET` territory, balance trending up.
- `acc-2` ("Joint"): spending consistently exceeds its small interest income —
  ends in `CashflowNodeKind.DEFICIT` territory, balance trending down.

A `cashflowGraph` response spanning both accounts in the same request
therefore has to show **one `NET` and one `DEFICIT` node side by side** — the
edge case §5 calls out explicitly, rather than something the FE mocks had to
invent independently of this corpus.

Each account also has **more than eight distinct spending counterparties**
across the period, so a `cashflowGraph` response truncates into one `Other`
node per account at the default `maxNodesPerDimension: 8` — and a couple of
transactions per account have neither `remitter` nor `creditor` named, landing
in the `Unknown` node (never dropped — §5).

## Edge cases exercised (§9 "done when")

- **Headerless phase-1-style record** (`ACC1-PHASE1-0001`): only
  `comdirect_account_key`/`comdirect_account_name`/`imported_at` are set, as
  `producer.rs` did before the envelope contract existed. Exercises
  `envelope::Envelope::parse`'s defaults for `source`/`origin`/
  `schema_version`.
- **Transaction missing `source_account_id`** (`ACC1-MISSING-ACCOUNT-0001`):
  otherwise fully headered, but the one header the projector cannot do
  without. Exercises the skip + log path (§2.2) rather than guessing an
  account.
- **Legacy-backfill reconstruction** (`LEGACY-0001`): `origin=legacy-backfill`,
  value is the reconstructed JSON from `payloads/legacy/`, not bank bytes —
  `is_bank_verbatim` must be `false` for this record.

## Regenerating

There is no committed generator — the corpus is deterministic, hand-curated
data checked in directly. If the shapes drift from `comdirect-rs`'s structs,
regenerate by hand (or a throwaway script) against the current structs in
`comdirect-rs/src/comdirect/{account_client,balance_model,transaction}.rs` and
re-derive the FE mocks from the new numbers.
