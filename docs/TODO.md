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

## Housekeeping

- **Delete `prompts/categories.json`** — XS. Dead since May 2026: nothing in
  the repo reads it, `category-seed` takes `prompts/taxonomy.json`, and its
  subcategory names disagree with the live ones (`child savings` vs
  `child_savings`). `CLAUDE.md` pointed at it until 2026-10-10. Left in place
  only because deleting it was never asked for.
- **Jumbled justfile comments** — XS. The comment block above `dev-fe-mocks`
  opens with "Run the frontend locally, tower profile (talks to the deployed
  Unraid backend)", which belongs on `dev-fe-tower`; that recipe now has no
  comment at all.
- **Two new topics are not applied** — S, blocks the deployed stack.
  `finreport.display-alias` and `finreport.transaction-link` exist in
  `terraform/kafka/main.tf` and locally, but no `terraform apply` has run
  against the central broker, so the deployed projector cannot consume them.
  Nothing from 2026-10-09 is deployed at all: it needs a redeploy too.

## Open debt

- Pre-existing prettier issue in `finreport-fe/src/lib/graphql/schema.graphql`
  (the mirrored SDL is not prettier-formatted).
- The root `README.md` still documents `sqlx-cli`; the project uses `sea-orm-cli`.
