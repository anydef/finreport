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

**Done 2026-10-08** — the categorisation/tagging/browsability batch:

1. ~~Spending by category excludes income and savings~~ — the dashboard now
   asks `categoryBreakdown` for `kind: EXPENSE`.
2. ~~Rolling 30-day period~~ — "Last 30 days", distinct from "This month",
   which was already month-to-date.
3. ~~Make the labeler able to clear a backlog~~ — `APP_llm_max_requests_per_run`
   defaulted to 200 *per process* and was never reset, and the long-running
   labeler only swept at startup, so it went silent after 200 uncached calls
   until someone restarted the container. It now sweeps on a timer
   (`APP_labeler_sweep_interval_secs`, default hourly), refills per sweep, and
   defaults to **no cap** (0 = unlimited). Sweeps and live records have
   separate budgets so neither starves the other.
4. ~~`uncategorized` means no category, not no label row~~ — it tested for the
   absence of a `transaction_label` row, so with every transaction labelled it
   matched nothing while the breakdown reported a large uncategorised total.
5. ~~Transaction detail modal~~ — click any row to edit category, tags and the
   recurring flag; shared `TransactionItem` extracted.
6. ~~Filters on every transaction table~~ — one shared panel on the dashboard
   and `/transactions`: accounts, categories, tags, amount range, search,
   tri-state flags, linkable via search params.
7. ~~Multi-select and bulk edit~~ — select rows or everything matching the
   filter, then override categories or tags from the table header. Filter-based
   mutations, so "all matching" means all of them and not the loaded page.

Still open:

8. Iteration 4: savings and spending goals (`docs/specs/iteration-4.md`).
   WP0 contracts and WP-C (UI on mocks, approved) are merged; WP-A (evaluation)
   and WP-B (GraphQL) are in progress.
9. Admin user management in the UI.
10. Bank connections via UI: credentials, sync, TAN status. **Blocked on a user
    decision about credential storage** (security-critical).
11. C24, PayPal and Scalable Capital integrations. First research the access
    method for each (API vs. CSV fallback).
12. Sankey cash-flow views.

### Deployment / data actions outstanding (user)

- Redeploy for the labeler and bulk-edit backend changes.
- Clear the empty-taxonomy residue so the labeler can re-label: delete the
  poisoned LLM cache entries (`DELETE FROM llm_label_cache WHERE category_id
  IS NULL`, ~148) and the stuck labels (`DELETE FROM transaction_label WHERE
  category_id IS NULL`, ~610), then restart the labeler. Canary one
  transaction first. Do **not** change `APP_projection_group` afterwards: the
  deleted rows still exist as records on `finreport.transaction-label`, and a
  projector replay would resurrect exactly what was deleted.

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
