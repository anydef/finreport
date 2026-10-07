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

1. Review iteration 3 against real data (`just dev-demo`, then the deployed stack).
2. Iteration 4: savings and spending goals (see requirements).
3. Transaction detail modal: edit category, splits, tags and the recurring
   flag from any list, via a shared `TransactionItem` + modal (see
   requirements). Frontend-only — the mutations already exist.
4. Dashboard transaction filters: account, category, tag, amount, flags —
   reusing `/transactions`' controls. Needs `amountMin`/`amountMax` added to
   `TransactionFilter`; everything else already exists (see requirements).
5. Spending-by-category drill-down: make the `uncategorized` and
   `needs-review` rows clickable (they are inert buttons today) and make the
   drill-down visible. Frontend-only (see requirements).
6. Transaction table search + multi-select + bulk tag/category edit. Needs
   backend work: bulk mutations, and tag *add/remove* rather than
   `setTransactionTags`' whole-set replace (see requirements).
7. Admin user management in the UI.
8. Bank connections via UI: credentials, sync, TAN status. **Blocked on a user
   decision about credential storage** (security-critical).
9. C24, PayPal and Scalable Capital integrations. First research the access
   method for each (API vs. CSV fallback).
10. Sankey cash-flow views.

## Open debt

- Pre-existing prettier issue in `finreport-fe/src/lib/graphql/schema.graphql`
  (the mirrored SDL is not prettier-formatted).
- The root `README.md` still documents `sqlx-cli`; the project uses `sea-orm-cli`.
