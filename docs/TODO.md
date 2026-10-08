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

**In progress (2026-10-08), making the tool natural to use rather than merely
complete:**

1. **Review queue groups by merchant.** 196 held transactions are nowhere near
   196 decisions — the same merchants repeat. The queue listed them
   individually, so the user paid per transaction instead of per merchant.
   Grouping by `counterparty_key` with a one-action assign per group is the
   single biggest reduction in effort available, and needs no new concepts:
   the key already drives rule learning and recurring detection, and the bulk
   mutations already take a filter.
2. **The dashboard says what needs attention** — uncategorised and held counts
   *and what they are worth*, linking into the filtered views. A person opening
   a finance app asks "is anything wrong?" before "what did I spend on food?",
   and today they have to remember to visit `/review`.
3. **A transaction row shows its own state** — a split indicator (the row shows
   the whole amount while totals count it by parts, which is actively
   misleading) and the label source, so a list can be scanned for what is
   curated versus guessed.

**Next:**

4. **"What changed?"** Period-over-period deltas: groceries up EUR 80 on last
   month, a new subscription appeared, a recurring charge stopped. Every number
   today is a snapshot of one period, while the question people actually ask is
   comparative. The recurring detector and the history are already there. This
   is the only item here that is new feature work rather than reshaping what
   exists, and it is what makes the dashboard worth opening when nothing is
   broken.
5. **Progress feedback while curating** — the uncategorised count falling, and
   "N rules learned from your decisions", so an hour of categorising reads as
   progress rather than a chore. Cheap once item 2's counts exist, and much
   more meaningful now that one human decision learns a rule.
6. Accounts as a multi-select dropdown in the filter panel (needs a
   multi-select mode on `SearchMenu`, which would also serve categories and
   tags there).
7. A Counterparty tab on the breakdown table, matching the chart's dimension
   tabs. Needs a backend decision: `categoryBreakdown` is category-specific,
   and `CashflowDimension` belongs to the Sankey. Generalising the breakdown to
   take a dimension probably beats a second query, since `TAG` is already
   declared in that enum.
8. Admin user management in the UI.
9. Bank connections via UI: credentials, sync, TAN status. **Blocked on a user
   decision about credential storage** (security-critical).
10. C24, PayPal and Scalable Capital integrations. First research the access
    method for each (API vs. CSV fallback).
11. Sankey cash-flow views.

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
