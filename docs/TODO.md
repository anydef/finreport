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

## Open debt

- Pre-existing prettier issue in `finreport-fe/src/lib/graphql/schema.graphql`
  (the mirrored SDL is not prettier-formatted).
- The root `README.md` still documents `sqlx-cli`; the project uses `sea-orm-cli`.
