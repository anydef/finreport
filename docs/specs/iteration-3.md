# Iteration 3 — tags, internal transfers, recurring costs

Three thin end-to-end slices, each visible in the UI: **free-form tags**,
**internal-transfer detection** (excluded from spending), and **recurring-cost
detection** with a `/recurring` page and a per-transaction override.

Iteration 2's shape is kept verbatim: Kafka is the source of truth, Postgres a
rebuildable projection, human decisions events on a compacted topic, and a user
override always beats auto-detection and is never clobbered by a re-run. Inputs:
`docs/requirements.md` (Tags; Internal transfers; Decision 2), `docs/architecture.md`,
`docs/specs/iteration-2.md` (§2 event model, §5 GraphQL, §6 FE, §10 WPs). Budget
is binding: one short blocking contracts package, then 3 parallel agents;
anything not needed to make the slices *visible* is deferred.

## 1. Scope

**In scope.** (1) Tags: free-form, user-owned, many per transaction, editable,
filterable, projected. (2) Internal transfers: deterministic pairing of opposite
amounts between accounts the same user owns; a badge; excluded from spending
totals. (3) Recurring: deterministic series detection; a `/recurring` page with
monthly-equivalent cost and a total; a per-transaction override the UI marks as
*you* vs. *auto*. One new pass, one new topic, two migrations, additive GraphQL,
one new route.

**Non-goals.** Auto-tagging; tag admin (rename/merge/colour/hierarchy); transfer
*category* assignment (detection sets a flag — iteration 2's category-kind
`transfer` keeps working and totals exclude either); recurring forecasting,
alerts or budget integration; goals; multi-currency; RBAC; dropping `legacy_*`.
`CashflowDimension.TAG` stays declared-and-rejected.

**Deferrals** — rules setting tags/recurring (`requirements.md` already says
"later" → iter 4); tag merge/rename/admin page (one user, ~10 tags; a typo is
fixed by re-tagging); a transfer confirm/reject override (exact-amount matching
is high precision — add it on the first false positive); fee-tolerant transfer
matching (tolerance buys false positives, not recall, on one bank — revisit with
a second source); series annotation (name, "cancelled") → iter 4 with goals;
transfer/recurring in the Sankey (the badge and `/recurring` are the visible
win).

## 2. Data & events

### 2.1 Tags and the recurring override reuse `finreport.user-label`

Both are human decisions about one transaction, keyed by it, last-writer-wins —
exactly what `finreport.user-label` already is. The record goes to
`schema_version: 2`:

```jsonc
{ "schema_version": 2, "source": "…", "external_id": "…",
  "category_slug": "food.restaurants|null", "parts": [ /* iter 2 */ ],
  "tags": ["italy-2026","hobby"],  // normalized, deduped, sorted; [] = none
  "recurring": true,               // true | false | null (null = no opinion)
  "revision": "RFC3339", "note": "string|null" }
```

A v1 record read as v2 means `tags: []`, `recurring: null`.

**The record is whole-state, so every mutation is read-modify-write.** The topic
compacts per transaction, so publishing `{tags}` alone would erase the category
and splits. `setTransactionTags`, `setTransactionCategory`, `splitTransaction`
and `setTransactionRecurring` each load the current `transaction_user_label` +
`transaction_tag` + splits from the projection, apply their one change, and
publish the complete record, with iteration 2's publish-then-upsert and
`revision` guard unchanged. The labeler already consumes `user-label`; a
tags-only or recurring-only edit re-resolves with unchanged inputs, so
compare-before-publish emits nothing — **no labeler change needed**.
*Rejected: a separate `finreport.tag` topic* — its own projection, offset row,
registration and replay ordering, for a field already part of the
per-transaction human-decision record.

### 2.2 New topic `finreport.transaction-insight`

Key `<source>:<external_id>`, compacted, partitions 1, RF 1, `prevent_destroy`,
iteration-1 envelope headers, `origin = detector`; a tombstone deletes the row.

```jsonc
{ "schema_version": 1, "source": "…", "external_id": "…",
  "is_transfer": true, "transfer_counterpart": "comdirect:XYZ|null",
  "transfer_match": "iban|amount_date|null",
  "is_recurring": false, "recurring_series_id": "uuid|null",
  "recurring_cadence": "monthly|quarterly|yearly|null",
  "recurring_median_amount": "-49.9900|null",
  "detected_at": "RFC3339", "revision": "RFC3339" }
```

Series *attributes* are denormalized onto each member record instead of a second
topic: `/recurring` is then a `GROUP BY recurring_series_id` over the read model,
and one topic means one projection handler.

**Auto vs. user, merged at read time.** The detector writes only the auto layer
(`transaction_insight`); the user writes only the override layer
(`transaction_user_label.recurring`). Nothing merges on write, so re-detection
cannot clobber an override and a replay cannot undo one:
`effective_recurring = user_label.recurring ?? insight.is_recurring`, and
`recurring_source = USER` iff `user_label.recurring IS NOT NULL`, else `AUTO`.
Transfers have no override layer here, so their source is always `AUTO`.

### 2.3 Read model

Two sea-orm migrations, then `make migrate && make generate-entities`.
Projections, no foreign keys (iter 2 §3).

**`m20261201_000001_tags`**
```
transaction_tag(transaction_id UUID NOT NULL, tag TEXT NOT NULL,
                revision TIMESTAMPTZ NOT NULL, PRIMARY KEY (transaction_id, tag))
CREATE INDEX idx_transaction_tag_tag ON transaction_tag (tag);
ALTER TABLE transaction_user_label ADD COLUMN recurring BOOL NULL;
```

**`m20261201_000002_insights`**
```
transaction_insight(transaction_id UUID PRIMARY KEY,
    is_transfer BOOL NOT NULL DEFAULT false,
    transfer_counterpart_id UUID NULL, transfer_match TEXT NULL,  -- iban|amount_date
    is_recurring BOOL NOT NULL DEFAULT false, recurring_series_id UUID NULL,
    recurring_cadence TEXT NULL,                       -- monthly|quarterly|yearly
    recurring_median_amount NUMERIC(20,4) NULL,
    detected_at TIMESTAMPTZ NOT NULL, revision TIMESTAMPTZ NOT NULL)
-- indexes: (recurring_series_id), (is_transfer)
```

The tag projection replaces a transaction's whole tag set in one statement pair
(`DELETE` what the record omits, `INSERT … ON CONFLICT DO NOTHING`), guarded by
`revision` like every other human-owned projection.

### 2.4 Configuration

| Key | Default | Notes |
|---|---|---|
| `APP_transfer_match_days` | `3` | inclusive window on `abs(Δ booking_date)` |
| `APP_recurring_min_occurrences` | `3` | per `requirements.md` |
| `APP_recurring_amount_tolerance` | `0.10` | relative to the series median |
| `APP_recurring_window_months` | `18` | detection lookback |
| `APP_max_tags_per_transaction` | `10` | mutation-side validation |

## 3. Detection

**Where it runs: a post-batch pass inside the existing `labeler` process**, in a
new `webapp/src/detect` module, invoked after the rule learner — the same "read
the projection, publish if changed" shape the learner already has (iter 2 §2.8).
No new binary, no new compose service, no new deploy IP, no new offset
bookkeeping; `--until-caught-up` and the deployed tailing service work unchanged.
*Rejected:* a new processor binary (a third always-on service and a fourth
`dev-demo` step for two pure functions) and running inside the projector
(detection needs *all* of a user's accounts, which a per-record batch lacks).

Both algorithms are **pure functions over owned plain structs**, no DB or Kafka
types in their signatures, so they unit-test directly:

```rust
pub fn detect_transfers(txns: &[TxnFacts], cfg: TransferConfig) -> Vec<TransferPair>;
pub fn detect_recurring(txns: &[TxnFacts], cfg: RecurringConfig, today: NaiveDate)
    -> Vec<RecurringSeries>;
```

`TxnFacts = { id, account_id, owner_user_ids, booking_date, amount: Decimal,
counterparty_iban: Option<String>, counterparty_key: Option<String> }`, loaded by
the processor with one query joining `transaction`, `account`, `user_account`.

### 3.1 Internal transfers

A pair `(a, b)` is a candidate iff: (1) `a.account_id != b.account_id`;
(2) the two accounts share **at least one owning user** (`user_account`);
(3) `a.amount == -b.amount` **exactly** at `NUMERIC(20,4)` and `amount != 0`;
(4) `abs(a.booking_date - b.booking_date) <= APP_transfer_match_days`.

Candidates rank by: `iban_confirmed` first (one side's `counterparty_iban` equals
the other side's account IBAN), then smaller date distance, then
`(min(id), max(id))` ascending. Walk that order, take a pair only if **neither**
side is already matched — a greedy, total, deterministic 1:1 matching.
`transfer_match` records the winning bucket (`iban` / `amount_date`). Both sides
get `is_transfer = true` plus each other's id.

**Edge cases.** *Different users / logins*: accounts with no common owner never
pair — money to someone else is a real outflow; two logins of the **same** user
do pair, since ownership not login is the criterion. *Shared family account*: one
common owner suffices. *Partial match* (a fee makes amounts differ): no match, by
design. *Several identical transfers in the window*: the ranking pairs them 1:1,
independent of row order or batch boundaries. *One leg not imported yet*: no
pair, nothing published for the lonely leg; it flips on a later pass. *A pair
that stops matching* (leg deleted or re-published with a new amount): the pass
publishes `is_transfer = false` — un-flagging is as mandatory as flagging.
*Same-account corrections* are excluded by (1).

### 3.2 Recurring costs

Group the last `APP_recurring_window_months` of a user's transactions by
`(counterparty_key, sign(amount))`; `counterparty_key` is iteration 2's
normalized key and a transaction without one is never recurring. Within a group,
sorted by `booking_date`:

1. `n >= APP_recurring_min_occurrences` (3).
2. **Amount**: `median = median(abs(amount))`; every occurrence must satisfy
   `abs(abs(amount) - median) <= max(tolerance * median, 1.00)`. Occurrences
   outside the band drop out, then (1) is re-checked.
3. **Cadence**: consecutive-date gaps must all fall in one cadence band —
   `MONTHLY` 26–35 d, `QUARTERLY` 83–98 d, `YEARLY` 350–380 d — where **at most
   one** gap may instead fall in that cadence's doubled band (one skipped
   period). Cadences are tried monthly → quarterly → yearly, first match wins; no
   match ⇒ not a series.
4. `series_id = UUIDv5(FINREPORT_NS, "recurring\0"+counterparty_key+"\0"+sign+"\0"+cadence)`
   — deterministic, so a re-run or replay reproduces it.

A series yields `{ series_id, cadence, median_amount (signed), occurrences,
first_date, last_date, next_expected_date = last_date + cadence }` and flags
every member. **Monthly equivalent** = `median_amount × {monthly 1, quarterly
1/3, yearly 1/12}`, exact decimal, half-up to 4 dp; the page total sums the
expense-direction series.

**Edge cases.** *Amount drift* (rent rises 3 %/yr): the band is relative to the
rolling median over the window, so slow drift stays one series and the median
follows; a step bigger than the band splits the history, the old occurrences age
out, and the series survives with the new amount. *Override then re-detection*:
the detector writes only `transaction_insight`, the override only
`transaction_user_label` — a re-run may flip flags freely and the user's answer
is untouched (§2.2). Marking one transaction recurring by hand does **not**
create a series; it flips that transaction's effective flag and badge. *A series
that stops*: members keep their flags, `next_expected_date` moves into the past
and the UI shows *stale*; no alerting. *An internal transfer that is also regular*
(monthly standing order to savings): both flags set, still never spending.
*Idempotence*: a record is published only when it differs from the projected row,
so a second pass over an unchanged read model publishes nothing.

## 4. GraphQL

Additive. Frozen by WP0 in `webapp/schema.graphql` and mirrored to
`finreport-fe/src/lib/graphql/schema.graphql`, as merged definitions in exporter
print order (iter 2 §5 — `extend` is spec shorthand).

```graphql
enum FlagSource { AUTO, USER }
enum RecurringCadence { MONTHLY, QUARTERLY, YEARLY }
enum TransferMatch { IBAN, AMOUNT_DATE }

# counterpartTransactionId is null while the other leg is not projected yet.
type TransferInfo { counterpartTransactionId: UUID, counterpartAccountId: UUID, match: TransferMatch! }

# source = USER when overridden (§2.2); seriesId null when overridden in/out of a series.
type RecurringInfo {
  isRecurring: Boolean!, source: FlagSource!
  seriesId: UUID, cadence: RecurringCadence, medianAmount: Decimal
}
type RecurringSeries {
  id: UUID!, counterpartyKey: String!, counterpartyName: String
  direction: Direction!, cadence: RecurringCadence!
  medianAmount: Decimal!            # signed
  monthlyEquivalent: Decimal!       # signed, 4 dp (§3.2)
  occurrenceCount: Int!, firstDate: Date!, lastDate: Date!, nextExpectedDate: Date!
  stale: Boolean!                   # lastDate older than cadence + grace
}
type RecurringOverview {
  series: [RecurringSeries!]!
  totalMonthlyEquivalent: Decimal!  # expense series only, positive magnitude
  currency: String!
}
type TagCount { tag: String!, transactionCount: Int! }

extend type Transaction {
  tags: [String!]!                  # sorted; [] when untagged
  transfer: TransferInfo            # null = not an internal transfer
  recurring: RecurringInfo!         # always present; isRecurring may be false
}
extend type Query {
  tags: [TagCount!]!                              # all tags, descending count
  recurringSeries(filter: TransactionFilter): RecurringOverview!
}
extend type Mutation {
  "Replaces the whole tag set. [] clears. Normalized + deduped server-side."
  setTransactionTags(transactionId: UUID!, tags: [String!]!): Transaction!
  "null clears the override and lets auto-detection decide again."
  setTransactionRecurring(transactionId: UUID!, recurring: Boolean): Transaction!
}
extend input TransactionFilter {
  tags: [String!]      # AND-ed: the transaction carries all of them
  recurring: Boolean   # matches the *effective* flag (§2.2)
  transfer: Boolean    # true = only transfers, false = only non-transfers
}
```

**Semantics.**

- **Totals exclude transfers.** `cashflowSummary`, `cashflowGraph` and
  `categoryBreakdown` drop any transaction whose effective transfer flag is true
  **or** whose category `kind = TRANSFER`, unless the caller explicitly passed
  `filter.transfer = true` — then they asked for transfers and get them.
  `transactions` is unaffected: the list shows everything, badged.
- **Tag normalization** (one table, mirrored by the FE helper): NFKC, lowercase,
  trim, collapse internal whitespace and `_` to `-`, strip anything outside
  `[a-z0-9-]`, collapse repeated `-`, trim leading/trailing `-`. Empty after
  normalization ⇒ dropped; length must be `1..=32`; more than
  `APP_max_tags_per_transaction` ⇒ `extensions.code = "TOO_MANY_TAGS"`;
  duplicates collapse silently.
- Both mutations are read-modify-write publish-then-upsert (§2.1) and return the
  fresh transaction. A transaction the caller cannot see is an error, not a
  no-op.
- `recurringSeries` honours the date/account/category parts of the filter when
  selecting members but always reports the series' full
  `firstDate`/`lastDate`/`occurrenceCount` — a one-month window must not make a
  yearly series look like a single payment.
- `CashflowDimension.TAG` still errors.

## 5. Frontend

Tailwind 4 utilities only, no component library, no `@apply`, no dark mode,
LayerChart only; badges are text + shape, never colour alone. Pure logic lives in
`src/lib/*.ts` under vitest; there is still no component-render test setup.

- **Transaction rows** (`/transactions`, dashboard table): a `TagChips` cell and
  an inline `TagEditor` (click `+`, type, Enter adds, Backspace on empty removes
  the last, Escape cancels) calling `setTransactionTags` with the full resulting
  set; normalization previewed client-side, re-applied server-side.
- **Badges**, reusing `Badge.svelte` and the `labelBadge.ts` pattern:
  `⇄ transfer`; `↻ recurring · auto` vs. `↻ recurring · you`, so an overridden
  flag is visibly distinct from a detected one (Decision 2). Clicking cycles
  `auto → recurring → not recurring → auto`
  (`setTransactionRecurring(true | false | null)`).
- **Filters** on `/transactions`: a tag multi-select fed by `tags` (AND-ed, with
  counts), a tri-state *recurring* toggle and a tri-state *transfer* toggle, all
  serialized into the existing URL query-param pattern.
- **`/recurring`** (new route under the `(app)` guard; nav entry added by WP0):
  one table — counterparty, cadence, median amount, monthly equivalent,
  occurrences, last / next expected, a *stale* pill — sorted by monthly
  equivalent descending, with the total in a `TotalsRow` above it; a row links to
  `/transactions` pre-filtered to that counterparty. No chart.
- Pure helpers under vitest: `src/lib/tags.ts` (normalize, dedupe, validate,
  limit) and `src/lib/recurringView.ts` (monthly equivalent and totals in exact
  decimal **via strings, never `number`**, sorting, staleness).

## 6. Work packages

WP0 is short and blocking; WP-A/B/C then run in parallel over **disjoint** file
sets, each in its own worktree
(`git worktree add ../finreport-worktrees/<branch> -b <branch>`).

### WP0 — Contracts — **XS (one agent, ~20 min)**

Mechanical only: no logic, no resolvers, no components. **Owns:**
`migration/src/m20261201_00000{1,2}_*.rs` + its `lib.rs` registration,
`entity/**` (regenerated), `webapp/src/kafka/insights.rs` (new:
`TOPIC_TRANSACTION_INSIGHT`, `InsightRecord`), the `tags`/`recurring` fields on
`UserLabelRecord` in `webapp/src/kafka/labeling.rs`,
`webapp/src/detect/{mod,transfer,recurring,processor}.rs` as `todo!()` stubs with
the §3 signatures, the `webapp/src/projection/mod.rs` dispatch entry for the new
topic (pointing at WP-A's module), the post-learner call site in
`webapp/src/bin/labeler.rs`, `utils/src/settings.rs` (§2.4 keys),
`webapp/schema.graphql` + `finreport-fe/src/lib/graphql/schema.graphql` (§4),
`finreport-fe/src/lib/graphql/{types.ts,queries.ts}`,
`finreport-fe/src/lib/graphql/mocks/{tags.json,recurring-overview.json}` plus the
new fields in `mocks/transactions.json`,
`finreport-fe/src/routes/(app)/+layout.svelte` (the `/recurring` nav entry), and
`webapp/fixtures/**` (a transfer pair, a 4-occurrence monthly series, a
3-occurrence quarterly series, one drifting-amount series).

*Done when:* `make migrate` is clean, entities regenerate, the workspace builds
with the stubs, the SDL drift test passes, `npm run check` passes, mocks validate
against the SDL.

### WP-A — Detection + projection — **M**

**Owns:** `webapp/src/detect/{transfer,recurring,processor}.rs`,
`webapp/src/projection/insights.rs` (new — the insight *and* tag projections),
`webapp/tests/detect_*.rs`, the `dev-demo` assertions in `justfile`,
`docker-compose.local.yml` + `terraform/kafka` (the one new topic),
`docs/architecture.md` (§1 diagram: the detector pass is real now).

The two pure functions; the pass that loads facts → detects → compares →
publishes; the projections with tombstones and the `revision` guard; and the
demo: WP0's fixtures already carry the cases, so this adds assertions that after
the loop at least one `transaction_insight` row has `is_transfer = true` and at
least one `recurring_series_id` has ≥ 3 members — failing loudly otherwise, like
the iteration-2 assertions. *Depends on:* WP0.

### WP-B — GraphQL — **M**

**Owns:** `finreport-rs/webapp/src/graphql/**`.
`Transaction.tags/transfer/recurring` with the auto/user merge; `tags`;
`recurringSeries` (the `GROUP BY` and monthly-equivalent arithmetic in exact
decimal); the three filters; both mutations with read-modify-write
publish-then-upsert and tag validation; the transfer exclusion in
`cashflowSummary` / `cashflowGraph` / `categoryBreakdown`.
*Depends on:* WP0 only — it queries the tables WP0's migrations create, valid
though empty until WP-A fills them.

### WP-C — Frontend — **M**

**Owns:** `finreport-fe/src/routes/(app)/recurring/**`,
`finreport-fe/src/routes/(app)/transactions/**`,
`finreport-fe/src/lib/components/{TagChips,TagEditor,RecurringBadge,TransactionTable}.svelte`,
`finreport-fe/src/lib/{tags,recurringView}.ts` + their tests.
Built entirely against WP0's mocks (`PUBLIC_USE_MOCKS=1`), never blocked on a
backend; reads `graphql/{types,queries}.ts` and `(app)/+layout.svelte` — frozen by
WP0 — without editing them. *Depends on:* WP0's SDL + mocks.

```
WP0 ██ (blocking, ~20 min)
    ├─> WP-A detection + projection  (webapp/src/detect, projection/insights.rs, justfile)
    ├─> WP-B graphql                 (webapp/src/graphql/**)
    └─> WP-C frontend                (finreport-fe routes, components, lib)
```

## 7. Testing

| WP | Level | What |
|---|---|---|
| WP0 | build | migrations apply and reverse; entities regenerate unchanged; SDL drift; `npm run check`; mocks validate against the SDL |
| WP-A | unit | transfers: exact amount only; window boundary at exactly N days; same account rejected; no common owner rejected; shared account accepted; IBAN-confirmed outranks amount-only; two identical candidate pairs match 1:1 under input reordering; zero amount rejected |
| WP-A | unit | recurring: exactly 3 qualifies, 2 does not; monthly/quarterly/yearly band boundaries; one skipped period tolerated, two not; an out-of-band amount drops the occurrence and can drop the series below 3; drift inside the band keeps one series; `series_id` stable across re-runs and input order; monthly equivalent exact at 4 dp |
| WP-A | integration | fixture replay flags the transfer pair and the series; a second pass publishes **zero** records; un-flagging publishes when a pair stops matching; a tombstone deletes the row; rebuild from offset 0 reproduces `transaction_insight` + `transaction_tag` identically; `dev-demo` assertions hold |
| WP-B | unit | tag-normalization table (case, unicode, separators, length bounds, dedupe, limit); effective-flag truth table (auto / user true / user false / cleared) |
| WP-B | integration | `setTransactionTags` preserves an existing category + split, and `setTransactionCategory` preserves tags (read-modify-write); `setTransactionRecurring(null)` restores the auto value; `spending` drops by exactly the flagged legs, and `filter.transfer = true` re-includes them; the tag filter AND-s; cross-user mutation denied; `recurringSeries` reports full series extent under a narrow date filter |
| WP-C | vitest | `tags.ts` normalization matches the server table case-for-case; `recurringView.ts` monthly equivalent, total, sorting and staleness exact via strings |
| WP-C | smoke | `/recurring` renders series + total from mocks; adding a tag shows the chip and the tag filter picks it up; the recurring badge shows `you` after a toggle |

Integration tests stay behind the `integration` feature and `just
test-integration`, run once at the end of a WP (iter 2 §9.3).

## 8. Assumptions

1. Tags and the recurring override ride on `finreport.user-label` v2; every
   mutation on that topic is read-modify-write (§2.1).
2. Detection is one pass in the labeler process, not a new service (§3).
3. Auto and user layers are stored separately and merged at read time — which is
   what makes "override, then re-detect" safe by construction (§2.2).
4. Transfer matching is exact-amount, same-owner, within N days: precision over
   recall. No tolerance, no override, no category assignment.
5. A series is identified by counterparty + sign + cadence with a deterministic
   UUIDv5 id, so replays and re-runs converge.
6. Totals exclude transfers unless the caller explicitly filters for them.
7. Tags are user-set only; nothing auto-tags in this iteration.

## 9. WP0 decisions (addendum)

WP0 froze the contracts layer. Where the spec left a mechanical choice
unstated, these decisions were made — later WPs should treat them as settled
unless they prove unworkable:

1. **Stub-resolver convention**: a new `not_implemented_iter3(field: &str, wp:
   &str)` helper was added alongside iteration-2's `not_implemented(field,
   wp: u8)` (hardcoded to numeric WPs/iteration-2.md) rather than generalizing
   the existing one, to avoid touching iteration-2 call sites. Used by all 5
   new stub resolvers (`Transaction.tags/transfer/recurring`,
   `Query.tags/recurringSeries`, `Mutation.setTransactionTags/
   setTransactionRecurring`).
2. **`projection/insights.rs`**: WP0 created a minimal stub (`project_insight`
   returning `Ok(())`) so the dispatch in `projection/mod.rs` compiles. The
   spec lists this file under WP-A's owned files for the real implementation
   — WP-A fills in this same file rather than creating a new one.
3. **`UserLabelRecord` schema versioning**: `CURRENT_SCHEMA_VERSION` stays `1`;
   the new `tags`/`recurring` fields use `#[serde(default)]` so v1 wire
   records still parse (`tags: []`, `recurring: None`). A
   `USER_LABEL_SCHEMA_VERSION_V2 = 2` constant is defined but not wired into
   any producer — bumping the wire version on actual v2 writes is left to
   WP-B, which owns the mutations that will produce them.
4. **Settings defaults**: all 5 new keys (`transfer_match_days`,
   `recurring_min_occurrences`, `recurring_amount_tolerance`,
   `recurring_window_months`, `max_tags_per_transaction`) use the values from
   the spec's table verbatim, no deviation.
5. **FE `TRANSACTIONS_QUERY`/`mocks/transactions.json`**: deliberately left
   untouched. `Transaction.tags/transfer/recurring` resolvers currently error
   with `NOT_IMPLEMENTED`; requesting them in every transaction fetch would
   error the whole query against a live `dev-be`/`dev-be-tower` backend before
   WP-B lands. WP-C should extend this query (and regenerate the mock) once
   the resolvers are real.
6. **`transaction_user_label.recurring` column wiring**: wired directly into
   `project_user_label`'s upsert in `projection/labeling.rs` by WP0 (not left
   dead), since the migration adding the column made it a compile-time
   requirement on the `ActiveModel` literal. This is WP0 touching existing
   projector logic rather than only adding new files — flagged here as the
   one exception to "disjoint files."
7. **Existing `UserLabelRecord{}` construction sites** in
   `webapp/src/graphql/labels.rs` (category-change mutations) got
   `tags: Vec::new(), recurring: None` to compile, each with a `// TODO(WP-B)`
   comment: these mutations currently drop any existing tags/recurring on
   every category change, a real bug WP-B must fix via read-modify-write
   (§2.1), now explicitly flagged rather than silently left.
8. **Detection call-site wiring**: `detect::processor::run_detection_pass` is
   invoked inside `labeling::processor::run()`, immediately after both of its
   existing `run_sweep(...)` call sites (the startup sweep and the
   catch-up-exit sweep), rather than after `run()` returns in
   `bin/labeler.rs` (impossible — `run()` consumes `db`/`publisher` by value
   and only returns in the `--until-caught-up` path). Since `run_sweep`
   internally calls the rule learner, this satisfies "invoked after the rule
   learner" and reuses the existing cadence without changing `run_sweep`'s
   signature (it has a third call site in `webapp/tests/labeler_postgres.rs`).
9. **Demo fixtures** (`webapp/fixtures/`): added a transfer pair
   (`ACC1-TRANSFER-OUT-01` / `ACC2-TRANSFER-IN-01`, -300.00/+300.00,
   2024-07-15, IBAN-confirmed via the outgoing leg's `creditor.iban` = acc-2's
   real IBAN — `Remitter` has no `iban` field so the incoming leg can only be
   matched by counterparty name/amount/date), a 4-occurrence flat monthly
   series (`ACC1-RECURRING-MONTHLY-0{1..4}`, Fitnessstudio PowerGym, -39.90),
   a 3-occurrence quarterly series (`ACC1-RECURRING-QUARTERLY-0{1..3}`,
   KFZ-Versicherung HUK, -187.43), and (beyond the minimum ask, to exercise
   the tolerance config) a 4-occurrence amount-drifting monthly series
   (`ACC1-RECURRING-DRIFT-0{1..4}`, Stromanbieter E.ON, -65.00 -> -70.00
   partway through). All use 2024 dates consistent with existing fixtures and
   reference keys that don't collide with the existing set.
