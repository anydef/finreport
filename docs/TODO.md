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

1. Review iteration 3 against real data — **done** (see `docs/specs/iteration-3.md`;
   the live-data pass found the missing `source_account_id` envelope header and
   the repair tool that recovers it).
2. Iteration 4: savings and spending goals (`docs/specs/iteration-4.md`).
   WP0 contracts merged; WP-C (UI on mocks) next, for review before WP-A/WP-B.
3. Admin user management in the UI.
4. Bank connections via UI: credentials, sync, TAN status. **Blocked on a user
   decision about credential storage** (security-critical).
5. C24, PayPal and Scalable Capital integrations. First research the access
   method for each (API vs. CSV fallback).
6. Sankey cash-flow views.

## Backlog

Recorded feature requests, not yet prioritised. The estimate is effort, the
note is what makes it worth more or less than its cost. Pull one into "Next"
by agreeing where it goes.

- **Spending-by-category drill-down** — S, frontend-only. The `uncategorized`
  and `needs-review` rows are rendered as buttons but do nothing; category rows
  already work. Cheapest item here and fixes something that currently looks
  broken, so a reasonable candidate to pull forward.
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

- **Transaction detail modal** — M, frontend-only. Edit category, splits, tags
  and the recurring flag from any list. Every mutation it needs already exists.
  Also extracts a shared `TransactionItem`, which the two items below both
  build on — doing it first makes them cheaper.
- **Dashboard transaction filters** — M. Mostly surfacing controls
  `/transactions` already has; the only backend work is `amountMin`/`amountMax`
  on `TransactionFilter`. Two open decisions (signed vs magnitude; whether the
  panel narrows the charts too).
- **Table search, multi-select and bulk tag/category edit** — L, needs backend
  work. The largest of these: bulk mutations, tag add/remove rather than
  whole-set replace, a selection model that means "all matching" not "this
  page", and non-atomic partial-failure handling. Highest leverage once the
  taxonomy is in use and many transactions need re-filing; least worth doing
  before then.

## Open debt

- Pre-existing prettier issue in `finreport-fe/src/lib/graphql/schema.graphql`
  (the mirrored SDL is not prettier-formatted).
- The root `README.md` still documents `sqlx-cli`; the project uses `sea-orm-cli`.
