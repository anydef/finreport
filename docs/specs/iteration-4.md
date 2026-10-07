# Iteration 4 — savings and spending goals

A goal is a threshold the user sets against a **scope** of transactions and
checks over time, with its own browsable page. One new topic, one projection,
additive GraphQL, two new routes.

Iteration 1–3's shape is kept verbatim: Kafka is the source of truth, Postgres
a rebuildable projection, human decisions are events on a compacted topic, and
a user decision is never clobbered by a re-run. Inputs: `docs/requirements.md`
("Savings and spending goals (iteration 4)"), `docs/architecture.md`,
`docs/specs/iteration-2.md` (§2 event model, §5 GraphQL, §6 FE),
`docs/specs/iteration-3.md` (§2.1 read-modify-write, §4 GraphQL shape).

## 1. Scope

**In scope.** (1) Goal CRUD as events: type, scope, threshold, period. (2)
Evaluation: per-period or cumulative totals against the threshold, computed at
read time. (3) A `/goals` list and a `/goals/[id]` page with a chart, totals
and the matching transactions.

**Non-goals.** Alerts or notifications; forecasting; rollover of unused budget
(requirements: "each period is evaluated on its own, with no rollover");
multi-currency; goal sharing between users; auto-suggested goals; goals over
investment positions (requirements Decision 3 — positions are still not
counted, only transactions that move money into investments).

**Deferrals** — goal templates; comparing two goals on one chart; a goal over
a recurring *series* rather than categories/tags (iteration 3 §1 already
deferred series annotation); progress-over-time of a `saving_target` across
periods (the fixed-range cumulative line covers the useful case).

## 2. Data & events

### 2.1 New topic `finreport.goal`

A goal is a human decision about how to read one's own money, keyed by its own
id, last-writer-wins — the same shape as `finreport.rule`. Compacted,
partitions 1, RF 1, `prevent_destroy`, iteration-1 envelope headers,
`origin = user`; a tombstone deletes the goal.

Key: the goal's UUID (as a string), like `finreport.rule`.

```jsonc
{ "schema_version": 1,
  "id": "uuid",
  "owner_user_id": "uuid",
  "name": "Hobbies",
  "type": "spending_limit",          // spending_limit | saving_target
  "amount": "200.0000",              // positive magnitude, NUMERIC(20,4)
  "currency": "EUR",
  "scope": {
    "category_slugs": ["leisure.hobbies"],   // each includes its descendants
    "tags": ["hobby"],
    "combine": "any",                        // all | any — category vs tag condition
    "tag_combine": "all"                     // all | any — within tags
  },
  "period": {
    "kind": "recurring",             // recurring | fixed
    "cadence": "monthly",            // monthly | quarterly | yearly; recurring only
    "start_date": "2026-01-01",      // fixed only
    "end_date": "2026-06-30|null"    // fixed only; null = open-ended
  },
  "archived": false,
  "revision": "RFC3339" }
```

**Whole-state, so every mutation is read-modify-write** (iteration 3 §2.1):
the topic compacts per goal, so a partial publish would erase the rest.
`updateGoal` loads the projected goal, applies its change, and publishes the
complete record with a fresh `revision`.

*Rejected: storing the scope as a join table on the event.* The scope is part
of the decision, not a separate entity with its own lifecycle, and nothing
resolves a goal through its scope members.

### 2.2 Read model

One sea-orm migration, then `make migrate && make generate-entities`.
Projection, no foreign keys (iter 2 §3).

**`m20270101_000001_goals`**
```
goal(id UUID PRIMARY KEY,
     owner_user_id UUID NOT NULL,
     name TEXT NOT NULL,
     goal_type TEXT NOT NULL,          -- spending_limit|saving_target
     amount NUMERIC(20,4) NOT NULL,
     currency TEXT NOT NULL,
     scope_category_slugs TEXT[] NOT NULL DEFAULT '{}',
     scope_tags TEXT[] NOT NULL DEFAULT '{}',
     scope_combine TEXT NOT NULL,      -- all|any
     scope_tag_combine TEXT NOT NULL,  -- all|any
     period_kind TEXT NOT NULL,        -- recurring|fixed
     period_cadence TEXT NULL,         -- monthly|quarterly|yearly
     period_start DATE NULL,
     period_end DATE NULL,
     archived BOOL NOT NULL DEFAULT false,
     revision TIMESTAMPTZ NOT NULL)
-- index: (owner_user_id, archived)
```

Arrays rather than child tables: the scope is read whole, every time, and
never joined against. `category_slugs` stores **slugs, not ids**, so renaming
a category does not invalidate a goal (iter 2's stability rule).

### 2.3 Evaluation is read-time, not a projection

No new processor and no stored progress. A goal's totals depend on the current
labels, splits, tags and transfer flags, all of which change as the labeler and
detector run; a stored total would be a cache to invalidate. Progress is
computed by SQL on request, like `cashflowSummary` (iter 2 §5).

This is the one deliberate departure from "everything is a projection": the
*goal* is projected, its *progress* is derived.

## 3. Matching

### 3.1 Contribution rows

Scope matching operates on **contribution rows**, not transactions: a
transaction with no valid split contributes one row (its own amount and
resolved category); a split transaction contributes one row per part (the
part's amount and category). This is what makes "split transactions count by
their parts" and "each transaction is counted once" both true.

A contribution row carries: transaction id, part index (null when unsplit),
amount, category slug (nullable), the transaction's tags, booking date.

### 3.2 Scope predicate

- **Categories:** a row matches if its category slug equals, or is a
  descendant of, any listed slug. Descendants are resolved from the `category`
  tree by slug prefix on the materialized path (categories are at most 3 deep,
  iter 2). Several categories combine with **OR** — a row has exactly one
  category, so AND could never match.
- **Tags:** `tag_combine = all` (default) requires the transaction to carry
  every listed tag; `any` requires at least one. Tags live on the transaction,
  not the part, so every part of a split inherits them.
- **Combining the two:** `combine = all` requires both conditions, `any`
  requires either. A scope with only categories, or only tags, uses just that
  condition and ignores `combine`.
- An empty scope (no categories, no tags) is rejected at mutation time, not
  silently matched against everything.

### 3.3 What counts

- **Internal transfers never count**, whatever the scope says — reuse
  `transfer_exclusion_sql` (iter 3 §4). Unlike `cashflowSummary` there is no
  opt-in to include them; a goal over transfers is meaningless.
- **`spending_limit`** sums **negative** contribution amounts as positive
  magnitude. A refund inside the scope is a positive row and **reduces** the
  total, per requirements.
- **`saving_target`** counts only rows whose category `kind = 'saving'`, and
  sums them as positive magnitude regardless of sign, so a transfer into
  investments counts as saving. Rows outside `saving` kind are ignored even if
  the scope names them.
- **Held for review** (`transaction_label.status = 'needs_review'`) is counted
  **separately** as `pending`, never folded into the total, per requirements
  ("shown separately as pending until they're resolved"). An unlabelled row
  with no category is pending too when the scope has a category condition; it
  is simply unmatched when the scope is tags-only.

### 3.4 Periods

- **Recurring:** buckets are calendar months, quarters or years intersecting
  the requested window, each evaluated independently. The bucket containing
  today is marked `in_progress` and never reported as a failure.
- **Fixed:** one bucket from `period_start` to `period_end` (or today when
  open-ended), with a cumulative series inside it for the chart.
- Bucket boundaries reuse iteration 2's `cashflow` bucketing rather than a
  second implementation.

## 4. GraphQL

Additive. `Goal`, `GoalProgress` and the mutations; nothing existing changes.

```graphql
enum GoalType { SPENDING_LIMIT, SAVING_TARGET }
enum GoalPeriodKind { RECURRING, FIXED }
enum GoalCadence { MONTHLY, QUARTERLY, YEARLY }
enum ScopeCombine { ALL, ANY }

type GoalScope {
  categories: [Category!]!      # resolved from slugs; archived ones still listed
  tags: [String!]!
  combine: ScopeCombine!
  tagCombine: ScopeCombine!
}

type Goal {
  id: UUID!
  name: String!
  type: GoalType!
  amount: Decimal!
  currency: String!
  scope: GoalScope!
  periodKind: GoalPeriodKind!
  cadence: GoalCadence          # null for FIXED
  startDate: Date               # null for RECURRING
  endDate: Date                 # null for RECURRING or open-ended FIXED
  archived: Boolean!
}

type GoalBucket {
  start: Date!
  end: Date!
  label: String!
  total: Decimal!               # positive magnitude
  pending: Decimal!             # held-for-review, excluded from total
  remaining: Decimal!           # amount - total; negative when over
  met: Boolean!                 # <= amount for a limit, >= amount for a target
  inProgress: Boolean!
}

type GoalProgress {
  goal: Goal!
  buckets: [GoalBucket!]!
  total: Decimal!               # across every bucket
  pending: Decimal!
  averagePerPeriod: Decimal!    # over completed buckets only
  currency: String!
}

extend type Query {
  goals(includeArchived: Boolean! = false): [Goal!]!
  goal(id: UUID!): Goal
  "Window defaults to the goal's own period for FIXED, last 12 periods for RECURRING."
  goalProgress(id: UUID!, startDate: Date, endDate: Date): GoalProgress!
  "The contribution rows behind a bucket, for drill-down."
  goalTransactions(id: UUID!, startDate: Date!, endDate: Date!, page: PageInput): TransactionPage!
}

extend type Mutation {
  createGoal(input: GoalInput!): Goal!
  updateGoal(id: UUID!, input: GoalInput!): Goal!
  archiveGoal(id: UUID!): Goal!
}
```

Every goal field is scoped to the caller: a goal belongs to the user who
created it (`owner_user_id`), and `goalTransactions` additionally restricts to
the caller's own accounts, like `transactions` (iter 2 §4). A cross-user id is
`null`/denied, never silently empty.

`GoalInput` mirrors the event's scope and period, validated at mutation time:
amount > 0; a non-empty scope; `RECURRING` requires `cadence` and rejects
`startDate`/`endDate`; `FIXED` requires `startDate` and rejects `cadence`;
`endDate >= startDate`; every `categorySlug` exists; tags normalized with
iteration 3's tag rules.

## 5. Frontend

- **`/goals`** — a card per goal: name, type, threshold, the current period's
  progress bar, and over/under colouring. An "Add goal" dialog using the
  existing `CategoryPicker` and a tag input.
- **`/goals/[id]`** — the goal page: recurring goals get bars per period with
  the threshold as a reference line (`CashflowBarChart`'s LayerChart setup,
  not a second chart library); fixed-range goals get a cumulative line against
  the budget line. Below it, totals (spent or saved, remaining, average per
  period, pending) and the matching transactions with drill-down, reusing
  `TransactionTable` and `Pagination`.
- Clicking a bucket narrows the transaction list to that bucket's range, the
  same `txStart`/`txEnd` search-param pattern the dashboard already uses.
- Pure logic in `src/lib/goalsView.ts` (bucket shaping, remaining/met
  arithmetic, progress-bar fractions), vitest-covered like `chartShaping.ts`.
- Nav gets a "Goals" tab in `(app)/+layout.svelte`.

## 6. Work packages

```
WP0 ██ (blocking, ~30 min)
    ├─> WP-A evaluation + projection (webapp/src/goals/**, projection/goals.rs)
    ├─> WP-B graphql                 (webapp/src/graphql/goals.rs)
    └─> WP-C frontend                (finreport-fe routes, components, lib)
```

### WP0 — Contracts — **XS**
The migration + regenerated entity, the `finreport.goal` topic in
`terraform/kafka/main.tf` **and** `docker-compose.local.yml`'s
`finreport-redpanda-init` (kept in step by hand, as always), the
`GoalRecord`/topic constant in `webapp/src/kafka/goals.rs`, the frozen SDL
additions with stub resolvers, FE types/queries/mocks, and a projection stub
registered in `projection/mod.rs`'s dispatch. Owns the contract; later WPs
fill bodies.

### WP-A — Evaluation + projection — **L**
`projection/goals.rs` (the real body), plus `webapp/src/goals/` holding the
contribution-row query, the scope predicate and the bucketing. The scope
predicate and the bucket arithmetic are **pure functions** over rows, tested
without a database; only the row query touches SQL.

### WP-B — GraphQL — **M**
`webapp/src/graphql/goals.rs`: the queries, the three mutations with
publish-then-upsert and read-modify-write, validation, and per-user scoping.

### WP-C — Frontend — **M**
The two routes, `goalsView.ts`, the goal cards and the goal chart. Built
against WP0's mocks (`PUBLIC_USE_MOCKS=1`), never blocked on a backend.

## 7. Testing

| WP | Level | What |
|---|---|---|
| WP0 | build | migration applies and reverses; entities regenerate unchanged; SDL drift; `npm run check`; mocks validate against the SDL |
| WP-A | unit | scope: category descendant match at each depth; several categories OR; `tag_combine` all vs any; `combine` all vs any; categories-only and tags-only scopes; empty scope rejected |
| WP-A | unit | counting: refund reduces a `spending_limit`; `saving_target` ignores non-saving kinds; transfers excluded; a split counts by parts and exactly once; needs-review lands in `pending` not `total` |
| WP-A | unit | buckets: monthly/quarterly/yearly boundaries; the current bucket is `inProgress`; `averagePerPeriod` excludes it; open-ended fixed range ends today; `met` at exactly the threshold (inclusive both ways) |
| WP-A | integration | goal record projects; a tombstone deletes it; replay from offset 0 reproduces `goal` identically; read-modify-write preserves an unedited field |
| WP-B | integration | cross-user goal id denied; `goalTransactions` restricted to the caller's accounts; validation rejects each bad input shape; `updateGoal` preserves fields it was not given |
| WP-C | vitest | `goalsView.ts` remaining/met/average arithmetic exact via strings; progress fraction clamped at 0 and 1 |
| WP-C | smoke | `/goals` renders cards from mocks; `/goals/[id]` renders bars and the threshold line; clicking a bucket narrows the list |

Integration tests stay behind the `integration` feature and `just
test-integration`.

## 8. Assumptions

1. A goal belongs to one user; no sharing, even for a shared account's
   transactions (requirements leave RBAC for later).
2. Progress is derived at read time, never stored (§2.3).
3. Scope matching happens on contribution rows, so splits and whole
   transactions share one code path (§3.1).
4. Internal transfers never count, with no opt-in (§3.3).
5. Held-for-review rows are `pending`, never silently counted (§3.3).
6. `saving_target` is restricted to `saving`-kind categories, per
   requirements; the scope cannot widen it.
7. Currency is whatever the goal records; no conversion. A scope spanning
   currencies is out of scope for this iteration.
