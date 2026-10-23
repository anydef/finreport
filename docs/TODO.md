# TODO

Requirements live in `docs/requirements.md`; per-iteration specs in `docs/specs/`.

## Status

| Iteration | Scope | State |
|---|---|---|
| 1 | Event-sourced MVP: Kafka → projector → Postgres → GraphQL → UI, auth, cash-flow views | merged, deployed |
| 2 | Categories, LLM labeling, rules engine, review queue, rules/category admin UI | merged, deployed |
| 3 | Tags, transfer detection, recurring detection (`docs/specs/iteration-3.md`) | merged; no final Opus review yet |

Deployment: frontend (`finreport.lab.anydef.de`, .50), one-shot migrate service (.51),
admin bootstrap (password in 1Password "finreport admin"), services on 192.168.100.45+.
See `docs/runbooks/iteration-1-deploy.md`.

## Next, in order

Agreed work, in priority order. Feature requests land in the Backlog below
first; they move up here only once their priority is settled.

Everything in this section came from the user working with their own data on
2026-10-08. The ordering principle: a **correctness bug beats a missing
feature**, because a wrong number is worse than an absent one; then whatever
reduces the 196-item review backlog, because that effort is being spent now;
then the things that make the tool worth opening when nothing is broken.

### Done 2026-10-08

- ~~Refunds net against charges in the category breakdown~~ — it summed
  `amount.abs()`, so a EUR 1000 medical bill reimbursed in full read as 2000
  spent instead of 0. It also disagreed with iteration 4's goal evaluation,
  which already netted refunds, so a goal and the breakdown gave different
  answers for the same two transactions.
- ~~Text search is case-insensitive~~ — the list used `LIKE` while the
  cashflow SQL used `ILIKE`, so the charts and the list disagreed about the
  same search term and the list silently dropped differently-cased rows.
- ~~One human decision learns an active rule~~, and ~~a user override deletes
  the contradicted LLM cache entry~~, so correcting a transaction propagates to
  its siblings instead of teaching the system nothing.
- ~~Spending by category excludes income and savings~~; ~~rolling 30-day
  period~~; ~~`uncategorized` means no category rather than no label row~~;
  ~~transaction detail modal~~; ~~filters on every table~~; ~~multi-select and
  bulk edit~~; ~~sortable tables~~; ~~rule conditions display and a matches
  count~~; ~~expandable subcategories~~; ~~the inert Uncategorized and Needs
  review rows drill down~~; ~~parent-prefixed category slugs~~ plus the
  `category-reslug` repair tool; ~~review-queue multi-select, bulk assign and
  find-similar~~; ~~the labeler's LLM cap removed~~ and ~~its slug validated
  against the taxonomy~~.

### Done 2026-10-09

- ~~Compare months, by category and in total~~ — a month/year picker
  (`preset=month:YYYY-MM`, linkable) plus a `/compare` route with per-category
  and total deltas, absolute and percentage, drilling into any cell. A category
  that appears or disappears reads "new"/"gone" rather than a meaningless
  +/-100%, and the current partial month is excluded by default, or
  October-so-far against September would read as a collapse in spending.
- ~~Free-text note on a transaction~~ — and it exposed a live data-loss bug:
  `setTransactionCategory`, `clearTransactionCategory`, `splitTransaction` and
  the bulk category edit all republished the whole-state record with
  `note: None`, so every category change or split would have erased the note.
  Fixed with the note preserved through all of them.
- ~~The "(no subcategory)" row is clickable~~ — via a new `categorySlugsExact`
  filter honoured by the list, the cashflow SQL twins, the breakdown, recurring
  and the bulk mutations.
- ~~Inline transaction editor~~ replacing the modal, with ~~in-place category
  creation~~ and ~~tag autocomplete~~ over `Query.tags` with usage counts.
- ~~Description-qualified rule learning~~ for ambiguous merchants, and
  ~~retiring the broad merchant-wide rule once narrow rules exist~~ — without
  which the first PayPal decision left a blanket rule labelling everything.

### In progress

1. **Review queue groups by merchant.** 196 held transactions are nowhere near
   196 decisions — the same merchants repeat. Grouping by `counterparty_key`
   with one assign per group is the largest reduction in effort available.
2. **Exempt a merchant from rule learning.** The safety valve for item 1 of the
   Done list above: one correction on a catch-all like Amazon or PayPal would
   otherwise create a counterparty-wide rule, and those are exactly the
   merchants the user wants to split by hand.
3. **The dashboard says what needs attention** — uncategorised and held counts
   and what they are worth, linking into the filtered views.
4. **A transaction row shows its own state** — split indicator (the row shows
   the whole amount while totals count it by parts) and label source.

### Next, in this order




1. **Progress feedback while curating** — the uncategorised count falling, and
   "N rules learned from your decisions". Cheap once item 3's counts exist, and
   meaningful now that one decision learns a rule.
2. Accounts as a multi-select dropdown in the filter panel (needs a
   multi-select mode on `SearchMenu`, which would also serve categories and
   tags there).
3. A Counterparty tab on the breakdown table, matching the chart's dimension
    tabs. Needs a backend decision: `categoryBreakdown` is category-specific
    and `CashflowDimension` belongs to the Sankey; generalising the breakdown
    to take a dimension probably beats a second query, since `TAG` is already
    declared in that enum.
4. Admin user management in the UI.
5. Bank connections via UI: credentials, sync, TAN status. **Blocked on a user
    decision about credential storage** (security-critical).
6. C24, PayPal and Scalable Capital integrations. First research the access
    method for each (API vs. CSV fallback).
8. Sankey cash-flow views.

### Deployment / data actions outstanding (user)

- Redeploy for the labeler, bulk-edit, goals, rules and sort changes.
- `finreport-be-category-reslug` now applies on deploy and repairs the six
  categories whose slug lacks their parent's prefix (and the 18 user overrides
  attached to them). Verify afterwards with the detection SQL in
  `docs/runbooks/category-reslug-repair.md`; once clean, that service and
  `finreport-be-repair-headers` can both be deleted — they are recovery, not
  steady state.
- Do **not** change `APP_projection_group` after deleting label rows: those
  records still exist on `finreport.transaction-label`, and a projector replay
  would resurrect exactly what was deleted.

## Backlog

Recorded feature requests, not yet prioritised. The estimate is effort, the
note is what makes it worth more or less than its cost. Pull one into "Next"
by agreeing where it goes.

- **Display nicknames for merchants and for own accounts** — S–M, needs a
  decision on scope first. The user: "I want to be able to give some merchants
  nicknames, e.g. all my accounts have my names on them, and it is hard to
  track which one is which." Two related but distinct wants hide in that
  sentence:
  - **Own accounts.** `Account.label` already exists and is documented "login
    label, display only" — but it comes from `APP_accounts__<n>__name`, so it
    labels the *login*, and every account that login imports gets the same
    string. Two accounts behind one Comdirect login are therefore
    indistinguishable, which is exactly the reported symptom. There is also no
    mutation to rename an account: the SDL has no account mutations at all. The
    fix is a per-account, user-editable nickname, which also fixes the
    transfer-counterpart display ("Eigenes Tagesgeldkonto" vs. the holder's
    name).
  - **Merchants.** A display alias per `counterparty_key`, shown wherever the
    raw bank counterparty is rendered, so "SumUp .Baeckerei Mueller" can read
    "Bakery". Keyed on the normalised key, so it covers every spelling variant
    at once — the same key the learner and recurring detection use.
  Both are the same shape: a user-declared alias, so an event on a compacted
  topic and a projection, like every other decision. Worth doing together
  rather than twice.

- **Spending trend for one category over time** — S frontend, no backend
  work expected. The user, 2026-10-24: "I want to be able to see the spending
  dynamic across individual category." Today `/compare` answers "which
  categories moved between these two months"; this asks the other question,
  "how has *this* category moved over the whole window", which nothing renders
  as a series.
  The data is already served: `categoryComparison` returns
  `periods` plus, per category, an amount per period — a time series per
  category, already fetched by `/compare`. So the first cut is a chart over a
  response the page already has (a sparkline in each table row, or a line
  chart for a clicked category), not a new resolver.
  Three things to settle before building, each a real limit of that query
  rather than a detail:
  - It defaults to `kind: EXPENSE`, so savings categories are absent — the
    same gap as the "/compare excludes savings" item below. A "spending"
    trend arguably should exclude savings, so this may be correct here; decide
    explicitly rather than inherit it.
  - `level: 1` means top-level categories. A trend for a *subcategory* needs
    `level: 2` or a scoped filter, and the user said "individual category",
    which more likely means a leaf than a top-level one.
  - A category with no transactions in a period must read as a zero, not as a
    gap — `/compare`'s table already distinguishes `new`/`gone`, and a line
    chart that interpolates across a missing month would invent spending.
  Worth more than its cost: it reuses a query that is already built, and it
  answers the question the breakdown and the comparison table both dance
  around. Depends on nothing else here.

- **Spending-by-category drill-down** — S, frontend-only. The `uncategorized`
  and `needs-review` rows are rendered as buttons but do nothing; category rows
  already work. Cheapest item here and fixes something that currently looks
  broken, so a reasonable candidate to pull forward.
- **"All matching except these" in bulk selection** — S backend, S frontend.
  Once "select all matching" is on, a row cannot be unticked: the bulk
  mutations take a `TransactionFilter`, which has `transactionIds` but no
  exclusion, so the only way back is to clear and re-tick by hand. Adding
  `excludeTransactionIds` to the filter closes it. Worth settling before more
  is built on the filter contract.

- **Phone layout for the web UI** — deferred deliberately. A native phone app
  is planned in Kotlin (user, 2026-10-08), so reshaping the SvelteKit tables
  into a phone card layout would be work the native app replaces. The detail
  modal already becomes a full-height sheet at phone width; a 7-column table
  does not, and is left as-is.

- **Clickable affordances** — S, frontend-only. Clickable things should look
  clickable: `cursor-pointer`, a hover state, and a visible focus ring. Today
  the frontend has only 3 `cursor-pointer` usages against ~10 components with
  click handlers (Sankey nodes, breakdown rows, badges, tag chips, review
  cards), so most click targets are indistinguishable from text. Cheap, and it
  cuts across every other UI item here, so it is worth doing early — ideally as
  shared classes or a small `clickable` helper rather than per-component
  one-offs. The inverse matters too: `CategoryBreakdown`'s `uncategorized` and
  `needs-review` rows currently *look* like buttons and do nothing, so
  affordance and behaviour must be fixed together (see the drill-down item).
- **Split transactions are invisible in the tables** — S, frontend-only.
  `TransactionTable` never renders a transaction's `splits`, so a split row is
  indistinguishable from a plain one — on the goal pages this is misleading,
  since goal totals count splits *by their parts* (iteration 4 §3.1) while the
  table shows the whole amount under one category. Needs at least a split
  badge and the parts on expand. Found while building goal fixtures.
- **Server-calculated running total for fixed-range goals** — S backend, S
  frontend. The Holiday-fund chart currently adds the transactions up in the
  browser, and it only fetches the first 200 of them, so a goal with more
  transactions than that draws a line that stops early. Moving the calculation
  to the server means adding a series of running totals to `GoalProgress` and
  having the goals resolver fill it. Accepted as good enough for now
  (2026-10-07); do this before any goal accumulates more than ~200
  transactions, which for a monthly goal is a few years.
- **Dynamic filter conditions on a goal's scope** — M. A goal's scope today is
  a fixed shape: a list of categories, a list of tags, and one choice of
  whether both must match or either. When both are given, both must match
  (confirmed 2026-10-07). The richer version lets the user build the condition
  themselves — nested groups of and/or, negation, and conditions on fields
  beyond category and tag. This replaces the `combine`/`tagCombine` flags with
  a small expression tree, in the event, the projection and the query, so it is
  worth doing deliberately rather than by growing the flags one at a time.
- **Dead-letter record for projector records that cannot be applied** — M. When
  the projector cannot map a record it logs the reason, skips it, and commits
  its position anyway, so the record is never revisited and the only evidence
  disappears with the container's logs. This is how 139 transactions and 832
  balance observations stayed missing for two months. Agreed approach
  (2026-10-07): write the skipped record into a table marked as failed, which
  makes it queryable and replayable, rather than a separate Kafka topic. Should
  carry the topic, partition, offset, key, headers, the raw value and the
  mapper's error, and a way to mark one resolved once it has been dealt with.

## Savings (2026-10-09/10)

The category side is done and merged. The account side is designed, previewed
and half-built; the rest is below. See the Savings section of `CLAUDE.md` for
the semantics, which the user decided and which should not be re-litigated.

- **Finish the savings-account backend** — L, high value. A Sonnet agent got
  partway and hit a session rate limit. Its snapshot is commit `e33b115` on
  branch `worktree-agent-a9f446fa3306daebc`, in the worktree
  `.claude/worktrees/agent-a9f446fa3306daebc`. **Assume it does not build** -
  it stopped mid-`graphql/accounts.rs` and was never compiled or tested.
  Present but unverified: migration `m20270401_000001_account_savings`, the
  `account_savings` entity, `graphql/savings.rs`, the topic in
  `terraform/kafka/main.tf` + `docker-compose.local.yml`, the projection
  dispatch, `just dev-reset` truncation, regenerated SDL, and a start on
  `Account.isSavings` plus integration tests. Still missing:
  `setAccountSavings`, the `savingsSummary` resolver and its SQL, unit tests
  for the netting, the `/admin/accounts` page, dashboard wiring, deleting
  `routes/(app)/savings-preview/` and `graphql/savingsMock.ts`, and making
  `Account.isSavings` required.
  Split it into three briefs, not one (storage / read side / UI wiring) - the
  single end-to-end brief is what ran long.
- **Card shape: decided 2026-10-23.** All four questions answered by the user,
  so slice 3 builds these rather than re-asking:
  1. **Headline stays as previewed** - net figure with both sides under it
     ("Put aside / 650.00 EUR / 900.00 in, 250.00 out").
  2. **Keep the overlap disclosure, reworded.** The previewed wording talked
     about cards rather than money; drop the second sentence and use one:
     `Includes 50.00 EUR spent directly from savings, which also counts as
     spending.` "Includes" is the load-bearing word - it says the headline
     figure already has that amount in it, which is the thing a reader
     otherwise gets wrong. Update `savingsOverlapNote` in
     `finreport-fe/src/lib/savingsView.ts` and its test.
  3. **Keep the excluded-internal-movement note** as previewed.
  4. **The savings toggle goes in the admin area** - a new `/admin/accounts`
     page, added to the `admin/+layout.svelte` sub-tabs next to "Nicknames".
  Still unasked: whether the per-account In/Out/Net table is the right level
  of detail. Built as previewed unless the user says otherwise.
- **`/compare` still excludes savings** — S. `CATEGORY_COMPARISON_QUERY` in
  `finreport-fe/src/lib/graphql/queries.ts` defaults to
  `$kind: CategoryKind! = EXPENSE`, the same default that hid savings on the
  dashboard. Left alone deliberately: comparing a savings contribution
  month-over-month may deserve its own column or card rather than silently
  joining the expense table, so decide the presentation before changing the
  default.
- **Goals ignore `TransactionFilter`** — S, known gap from the reimbursement
  work. Goals use their own scope, so goal progress still counts
  reimbursement-side money and will count savings flows too. Not covered by
  any test.

## Requested 2026-10-23

- **Navigate from a transaction to the rule that categorised it** — S, good
  value. When a label came from a rule, the user wants to jump from the
  transaction to that rule. The data is already there: `transaction_label`
  stores `rule_id` (nullable, set when `label_source` is the rule path), so
  this needs no migration and no new event. What is missing is exposure and a
  link - `ruleId` appears in neither `webapp/schema.graphql` nor
  `finreport-fe/src/lib/graphql/queries.ts` today.
  Shape: expose the rule on the label (prefer the rule object over a bare id,
  so the UI can show its name without a second round trip, and prefetch it per
  page rather than per row - see `graphql::links::prefetch_page`), then link
  from the transaction's editor/detail view to `/admin/rules` scoped to that
  rule. Worth deciding whether a deleted rule should still resolve: `rule_id`
  can point at a rule that no longer exists, and the link should degrade to
  naming it rather than 404.

## Savings: open items after slices 1-2 (2026-10-23)

- **Mixed-currency savings accounts hard-error** — M, latent. If the in-scope
  savings accounts span more than one currency, `savingsSummary` returns a
  `VALIDATION` error ("savings accounts span several currencies (EUR, USD);
  select accounts of one currency"), so the savings card shows an error rather
  than a figure on an otherwise working dashboard. The resolver never sums
  across currencies, which is right; the harsh part is the failure mode, since
  holding a foreign-currency account is not a user error.
  **Confirmed not an issue today: the user holds EUR only** (2026-10-23), so
  this is parked rather than fixed. Options when it matters: per-currency rows
  in the card, or a `currencies` field so the card can say "showing EUR only".
  The choice was deliberate - the schema has no field to disclose a silently
  dropped account, so erroring beat lying. Slice 3 must still fail gracefully
  rather than blanking the dashboard.
- **`setAccountSavings` success path is untested** — S. Only the rejection path
  has a test; the publish-then-project path has no broker in that suite. Same
  shape as `setDisplayAlias`, but untested is untested.
- **The summary has no filter tests of its own** — S. `transfer`, `direction`
  and `categorySlugs` reach it through the shared list predicate
  (`build_condition_with_category_filters`), so they are covered by
  construction, not directly. A filter regression would surface on the list
  before the card.
- **Terraform and compose for `finreport.account-savings` unvalidated** — S.
  No `terraform validate` and no compose run against the new topic. Note the
  related find: on `main`, `docker-compose.local.yml` had been creating
  `finreport.display-alias` with no `--partitions`/`--topic-config`, so
  locally that topic was created **without compaction**, diverging from
  Terraform. Fixed in slice 1; worth a sweep for others like it.

## PayPal: paused 2026-10-23

Paused by the user in favour of finishing the Amazon ingest, where the data is
real and already in hand. The spec `docs/specs/iteration-5-paypal.md` stands;
nothing about it was found wrong.

- **Why it stopped, not just that it did.** §4 of the spec asks for a recorded
  sandbox payload, on the reasoning that "the mapper is where a provider's
  surprises surface". The user has no PayPal sandbox account, so any fixture we
  write is hand-made from the documented response shape: it proves our logic
  self-consistent, not that PayPal behaves as documented. Building the mapper
  against synthetic payloads means every field PayPal returns differently
  surfaces on the first live run instead of in tests.
- **Where the work is.** Commit `7ad6e7e` on branch
  `worktree-agent-a079872cd80f0ce44`: 571 lines of crate skeleton
  (`window.rs`, `watermark.rs`, `token.rs`, `error.rs`, `lib.rs`) plus the
  workspace member entry. **Never compiled or tested** — the agent was stopped
  partway. No mapper, no projector wiring, no binary, no credential store.
- **Three ways to resume**, in rough order of cost:
  1. A **free developer app** at developer.paypal.com gives sandbox
     credentials in minutes and satisfies the spec's own testing requirement.
     This is the cheapest unblock.
  2. **CSV export** from PayPal's web UI instead of the API: real data the user
     can produce today, no OAuth, no windowing, no watermark, no deployed
     service, and it reuses the Amazon upload path. A different design from the
     spec, and manual per import rather than automatic.
  3. **Build the API path untested** and accept that the first real run is the
     first real test.
- Also still open from the spec: which `transaction_status` values to import
  and how a status change is represented without creating two rows; whether
  refunds need special handling (verify, do not assume); whether to publish one
  `account-balance` per currency; and that the 3-year limit makes a first
  import incomplete by nature, which belongs in the runbook.

## Amazon ingest: open after slice 2 (2026-10-23)

- **The upload publishes the wrong schema version in its headers** — S, do
  before this ships. The mutation reuses `events::publish_event`, whose headers
  carry `kafka::labeling::CURRENT_SCHEMA_VERSION` — the *labeling* schema
  version, not the purchase-order one. Harmless today because the two happen to
  agree in meaning to nobody, but it actively undermines the mechanism we are
  relying on: the record wraps `amazon_orders::Order` unchanged, so its serde
  shape is the wire contract, and a `schema_version` bump is the only defence
  against a field rename breaking stored records. A bump that does not reach the
  header defends nothing.
- **Amazon charges are found by text match** — M. The match report selects
  candidate bank lines with an ILIKE on `amazon`/`amzn` against description or
  counterparty. A charge that carries an order id but neither word is invisible
  to the report, and would look like "no bank charge" for an order that in fact
  matched. Comdirect lines do carry AMZN in practice, so this is a latent gap
  rather than a present bug. Consider also matching on a description containing
  something shaped like an order id, which `classify_description` already
  recognises.
- **One unexplained integration failure** — S, worth one deliberate look. During
  slice 2 a full `--features integration` run reported 728 passed, 1 failed; the
  agent did not capture which test, and two later runs were clean at 729. It is
  probably the shared-database flakiness this repo already sees under parallel
  worktrees, but "probably a flake" is not evidence. If it recurs, capture the
  test name before rerunning.
- **The review page and the real file picker are visually unverified** — S.
  Type-checks pass and the mock path has unit tests, but no browser has rendered
  `/admin/amazon`, and the actual file-input upload has never been exercised.
  Review with `just dev-fe-mocks`.

## Amazon apply: findings after slice 3 (2026-10-23)

The feature is built, merged and tested (554 unit / 751 integration). The
problem is not correctness — it is how rarely it will fire.

- **Most real orders can never be applied** — L, and the single thing that
  decides whether this feature earns its keep. `match_orders` returns `Apply`
  only when the items sum **exactly** to the order total; otherwise it is
  `OrderItemsDoNotReconcile` and `NeedsReview`
  (`amazon-orders/src/matcher.rs`). Shipping, gift cards and promotions all
  create a remainder, so an order carrying any of them is permanently
  unappliable. `build_parts` can emit an explicit remainder part — spec §2.4 —
  but nothing can reach that code, because the matcher refuses the order first.
  Either the matcher must learn to treat a remainder as a legitimate part
  (shipping is a real cost worth its own split), or the apply path has to accept
  a reviewed `NeedsReview` case. Decide this before building more on top.
- **A single-item order cannot be split at all** — M. `labels.rs:624` requires
  at least two parts, which is correct for splitting in general, but it means a
  one-item order shows as blocked rather than applied. Most Amazon orders are
  one item. The honest answer may be that a single-item order needs no split -
  only a category - in which case the page should offer that instead of a
  blocked row.
- **`purchase_order_split` is not a Kafka projection** — M, architectural. Every
  other piece of state in this repo is projected from a topic; this table is
  written directly, so it cannot be rebuilt by a replay and `just dev-reset`
  does not truncate it. Stale rows are detected and distrusted, which is a
  mitigation rather than a fix. Decide deliberately whether to keep the
  exception or move it onto a topic.
- **The entity was hand-written** — S. `CLAUDE.md` says entities are generated
  with `sea-orm-cli` and not hand-edited; this one was written in the generated
  style instead. Regenerate it so the next `make generate-entities` does not
  produce a surprising diff.
- **`splitTransaction` silently overwrites a user's split and pinned category**
  — M, pre-existing and not introduced here. Slice 3 found it while guarding
  against it, and put the check in the apply layer. The underlying mutation is
  still happy to discard a manual split if called directly, which is a sharp
  edge for any future caller.
- **No integration test covers a multi-charge `Apply` order** — S. The
  per-charge code path is shared, so it is covered by construction rather than
  directly.
- **A stale third SDL copy** exists at `webapp/graphql/schema.graphql` — XS. Two
  copies are kept in step by `sdl_drift`; this one is not and was left alone.
  Delete it or bring it under the drift test.
- **Spec §2.4's "route item titles through the labeling chain" is unbuilt** — M.
  A split part today carries an amount and a user-chosen category. Item titles
  are kept in `purchase_order_split` for display only, and nothing categorises
  them, so the user picks a category per item by hand on every apply.

## Housekeeping

- ~~Delete `prompts/categories.json`~~ — done 2026-10-23. Dead since May 2026.
  `docs/runbooks/dangling-llm-category-repair.md` still pointed at it and now
  names `taxonomy.json`. `finreport-fe/src/lib/graphql/mocks/categories.json`
  is a different, live file and was not touched.
- ~~Jumbled justfile comments~~ — done 2026-10-23. The mocks comment block now
  describes `dev-fe-mocks`, and `dev-fe-tower` has its own comment again.
- **Two new topics are not applied** — S, blocks the deployed stack.
  `finreport.display-alias` and `finreport.transaction-link` exist in
  `terraform/kafka/main.tf` and locally, but no `terraform apply` has run
  against the central broker, so the deployed projector cannot consume them.
  Nothing from 2026-10-09 is deployed at all: it needs a redeploy too.

## Open debt

- Pre-existing prettier issue in `finreport-fe/src/lib/graphql/schema.graphql`
  (the mirrored SDL is not prettier-formatted).
- The root `README.md` still documents `sqlx-cli`; the project uses `sea-orm-cli`.

## Rules clarity and safety (2026-10-23)

- **A rule with no conditions matches every transaction** — S, do before more
  hand-written rules exist. `graphql/rules.rs`'s `validate_conditions` rejects
  unknown keys, a bad regex and a bad direction, but accepts `{}`.
  `labeling/rules.rs`'s `count_matching` states the consequence outright: "A
  rule with no conditions at all matches every input." Learned rules always set
  `counterparty_key`, so only a hand-created rule can trip this. Reject an
  empty condition set at write time.
- ~~Learned rule names read like free-text patterns~~ — done 2026-10-23 by
  showing the condition, not by renaming: the stored name is a display string
  and renaming it would not touch rules already published. `/admin/rules`
  already printed `counterparty = "<key>"` per rule; `/review`'s pending-rule
  list now prints the same summary under the name (`conditions` was already in
  `RULE_FIELDS`, so no query change), and an empty condition set shows a
  warning badge — "no conditions — ignored, edit to restore" — instead of grey
  text that read like "matches anything".

## Amazon follow-ups (2026-10-23)

- **No column aliases** — S. The parser now resolves columns by name and
  ignores unknown ones, so order and extra columns are fine, but a *renamed*
  required column (e.g. `Item ASIN` -> `ASIN`) still rejects the file whole.
  The message names the missing column, so it is diagnosable; add aliases only
  when a real export shows one.
- **`first_number` on promotion text is a guess** — XS. Checked against a real
  header but not against real promotion values; a currency symbol or a
  different format may read wrong.
- ~~The upload outcome always says "No transaction was changed or split."~~ —
  done 2026-10-23. `uploadOutcome` now adds that line only when an order was
  actually read, and falls back to "Nothing was imported." rather than ever
  returning an empty message.
- **Locale-formatted bare amounts are mostly handled** — nil. `parse_decimal`
  reads `1.234,56` and `5,98`; `1,234` is ambiguous and is read as a thousands
  separator.

## Housekeeping (2026-10-23)

- **~110 merged branches are still around** — XS. `git branch --merged main`
  lists every `*-integrate`, `docs/*`, `fix/*` and `worktree-agent-*` branch
  from previous iterations. Not deleted because nobody asked; a sweep of
  `--merged` branches is safe but is the user's call.
- **`docs/comdirect_rest_api.postman_collection.json` holds real account data**
  — S, privacy. Saved example responses carry real IBANs, counterparty names,
  family members' names and ~6 months of real transactions. Pre-existing and
  committed long ago, so the history carries it too; scrubbing the file only
  helps future readers. Worth deciding on deliberately.

## Decided: an unreconciled Amazon order stays unapplied (2026-10-23)

The user's call, when asked whether to make a `NeedsReview` order applicable
by treating the leftover as its own part: **leave it hanging.** Their reasoning,
verbatim: *"no transaction -> no expense. Counting transaction and amazon
unmacthed order is a double expense."*

This affirms the invariant already in CLAUDE.md — orders are evidence about
transactions, never transactions themselves. An order that cannot be tied to a
bank line must therefore contribute nothing to spending, and the review surface
must never imply otherwise.

Consequences, accepted deliberately:

- ~~An order whose items do not sum exactly to the order total can never be
  applied.~~ **Revised 2026-10-24, with the user**, and this consequence no
  longer holds. Most real orders were in this state (shipping, a gift card, a
  promotion, rounding), so Amazon purchases never reached the report under any
  category — which is the opposite of what the user wants.
  A single-charge unreconciled order is now planned: the items get their parts
  and the leftover becomes `ChargePlan::remainder`, which `build_parts` turns
  into the "Shipping & adjustments" part (that path is now reachable). The user
  picks its category per order — their choice between a fixed category, a
  pro-rata spread and a per-order pick.
  **This is not the double counting the user rejected.** The parts still sum
  exactly to the bank transaction, so total spending is unchanged; only its
  division between categories changes. The rejected case was an order with *no*
  bank line, which would invent an expense. That case is untouched: an
  unmatched order is still never planned.
  Still `NeedsReview`: a multi-charge unreconciled order, and a **negative**
  remainder (items exceed the charge), which is a parse or data error rather
  than shipping.
- ~~A single-item order cannot be split at all~~ — done 2026-10-24. Such an
  order needs a category, not a split, and now gets one: a charge whose only
  non-zero part is a single item is applied by pinning that item's category
  (`applied_as` = `CATEGORY`), recorded in `purchase_order_split` as a one-part
  record so it stays idempotent and reversible. `labels.rs:624` is unchanged —
  a one-part split is still refused, because a one-part split says nothing a
  category does not.
  Known rough edge: the page's button label is chosen from the *order's* part
  count while the server decides per *charge*, so a multi-charge order whose
  charges each hold one item reads "Apply split…" and then reports "Category
  set". The SDL carries no order-level hint. XS, cosmetic.
- The review list is informational: reason, totals, charges, no control. That
  is now intended behaviour, not a gap. Do not "fix" it by making unmatched
  orders applicable.

## Amazon item categories are suggested automatically (2026-10-24)

Built, in four merged slices, after the user said plainly what they were
after: *"I want my amazon purchases to be associated with the real
transactions and have displayed in the actual report and appear in the proper
categories."*

- `uploadPurchaseOrders` runs the proposal engine over new or changed orders
  only, so a re-upload is free; answers land in the LLM cache.
- `purchaseOrderMatches` returns `suggestedCategorySlug` / `suggestionSource`
  per item, resolved from the **rule engine and the cache only**. A query
  resolver never calls a provider: opening a page must not spend money or
  block on the network. 3 extra queries per page, whatever the order count.
- A failed suggestion never fails an import: it is a `warn`, and the orders
  still import.
- The page prefills only empty pickers, and a user's own choice is never
  overwritten — not by a re-render, a refetch, or a forced "Suggest again".

Open, deliberately not fixed in those slices:

- **Three independent LLM budgets** now: the labeler, the per-order mutation
  and the upload. `APP_llm_max_requests_per_run` bounds none of them in
  total. One worse in count than the 2026-10-23 finding, not in kind.
- ~~The `fake` provider answers.~~ Closed 2026-10-24: `warm_inner` returns
  early when `APP_llm_provider` is `fake`, so an import never stores invented
  categories against real orders. Rules still answer on the read path, and the
  per-order mutation is still available for a deliberate run — it is unchanged,
  so pressing the button with a `fake` provider still caches fake answers. That
  is a decision the user makes per order, not one an import makes for them.
- **`ProposalSource::LLM` cannot occur on the read path** (it resolves from
  rule and cache only), so the "Suggested by AI, check it" badge is reachable
  only through the mutation. The frontend mock fakes one so the badge can be
  reviewed; that is mock-only.
- **No integration test covers warm-on-upload or the new fields.** The
  existing upload and matches tests pass but never exercise a provider or a
  populated cache.

## Labeling-engine findings from the proposal slice (2026-10-23)

- **The labeler never published the LLM cache to Kafka** - fixed. Answers and
  contradiction tombstones now go through `labeling::cache_publish`
  (publish, then project), shared with the proposal mutation. A publish failure
  is logged at `error` and still projected, like labels; a replay would then
  lose that one entry, which is refilled by the next LLM call.
- **Two independent LLM cost guards** — S. The labeler's guard is per process;
  the proposal mutation builds a fresh `CostGuard` per call, so the two spend
  against separate budgets and `APP_llm_max_requests_per_run` no longer bounds
  total spend. Budget exhaustion is at least visible: the item returns `NONE`
  with `reason: "llm_budget_exhausted"` plus one `warn`.
- **The item fingerprint is not item-specific** — nil, noted. A bank line whose
  normalised counterparty equals an item's normalised title (with an empty
  description) shares the cache row. Acceptable, but it is an overlap.
- **A rule with only a `direction` condition would match every item.** Harmless
  now that empty-condition rules are refused everywhere, but the same shape as
  the bug fixed in `41b4ad3`.
