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
