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
- `payloads/labels/<key>.json` — our own JSON (not a bank payload) matching
  `webapp::kafka::labeling::UserLabelRecord` (§2.6): the "already overridden"
  and "split" iteration-2 demo cases below.
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

## Labeling demo cases (§7, iteration 2)

Five counterparties on `acc-1`, keyed to exactly match `categorizer`'s
`FakeProvider` keyword/ambiguous/proposed-path tables (§2.9) once
`labeling::normalize` lowercases a clean `holderName` with no legal-form
suffix to strip:

- **Learning threshold** (`ACC1-LABEL-FITNESS-{01,02,03}`, `Fitness First`,
  3 occurrences): `APP_rule_learn_min_observations` defaults to 3, so this is
  the minimum repeat count that lets the learner (§2.8) propose a rule for
  `personal.gym` once the fake provider has labelled all three at its
  committed `0.88` confidence (below the `0.9` auto-approve threshold ⇒
  `state=in_review`, surfaced in the review queue).
- **Ambiguous** (`ACC1-LABEL-AMAZON-01`, `Amazon`): the fake provider's
  `AMBIGUOUS_KEYS` always resolves `review_reason=ambiguous`.
- **New-category proposal** (`ACC1-LABEL-ACME-01`, `Acme Co-Working`): the
  fake provider's `PROPOSED_PATH_KEYS` proposes `housing.coworking`, a slug
  deliberately absent from `prompts/taxonomy.json` — proposals are never
  auto-created (§2.5).
- **Already overridden** (`ACC1-LABEL-OVERRIDDEN-01`, `Imbiss Ecke` +
  `payloads/labels/acc1-label-overridden-01.json` on `finreport.user-label`):
  a user override published *before* the labeler ever sees the transaction,
  so the resolution chain's step 2 (§2.5) must leave it alone regardless of
  what the fake provider would otherwise say about `Imbiss Ecke`.
- **Split** (`ACC1-LABEL-SPLIT-01`, `Einkaufszentrum Mitte`, `-60.00` +
  `payloads/labels/acc1-label-split-01.json`): two parts (`-40.00`
  `household_items_supplies.cleaning`, `-20.00` `personal.cosmetics`) summing
  exactly to the transaction amount, per §2.6's no-tolerance rule.

## Regenerating

There is no committed generator — the corpus is deterministic, hand-curated
data checked in directly. If the shapes drift from `comdirect-rs`'s structs,
regenerate by hand (or a throwaway script) against the current structs in
`comdirect-rs/src/comdirect/{account_client,balance_model,transaction}.rs` and
re-derive the FE mocks from the new numbers.
