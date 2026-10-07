# Iteration 2 — categories, labeling and the review queue

Goal: every transaction carries a **category**, the user sees spending broken
down by category (chart + Sankey + filters), and an admin section lets them
correct what the machine decided — override a label, split a transaction,
manage rules, resolve what the labeler held back.

Iteration 1 made Kafka the source of truth and Postgres a rebuildable read
model. Iteration 2 keeps that shape: the labeler is **another Kafka processor**,
and everything a human decides is **another event on another topic**.

Authoritative inputs: `docs/requirements.md` (Categories / LLM providers /
Rules engine sections), `docs/architecture.md` (§2 label resolution, §3 learned
rule lifecycle, §4 review flow), `docs/specs/iteration-1.md` (contracts this
spec extends), root `CLAUDE.md`.

---

## 1. Scope

**In scope**

1. Category tree: ≤ 3 levels, `kind ∈ {income, expense, transfer, saving}`,
   stable ids and slugs, seeded from a committed JSON taxonomy keeping the
   German-specific items (Rundfunk, Kfz-Steuer).
2. `labeler`: a Kafka-native, idempotent processor resolving a category per
   transaction through **user override > rule > llm-cache > llm**, recording
   which link of the chain produced it.
3. Pluggable LLM providers behind one trait: Anthropic, Ollama, any
   OpenAI-compatible endpoint, plus a **deterministic fake** for tests and the
   local demo (no API key, no GPU, no network).
4. Rules engine: user-authored and **learned** rules, with confidence,
   auto-approval above a threshold, revocation.
5. Review queue: ambiguous labels, low-confidence rule candidates and
   LLM-proposed new categories, resolved in an admin UI.
6. User override layer: per-transaction override and **split transactions**
   (parts sum exactly to the total).
7. GraphQL: category tree, labels on transactions, a category breakdown,
   `CATEGORY` as a `cashflowGraph` dimension, category filters, review-queue
   and rule/override mutations.
8. SvelteKit: breakdown + category Sankey + category filters on the existing
   pages, and a new `/admin` area (review, rules, categories, splits).

**Out of scope**: tags, internal-transfer and recurring detection (iteration 3);
goals; multi-currency; RBAC and per-tenant isolation beyond the column reserved
in §3; labelling investment positions; dropping the `legacy_*` tables.

**Constraints carried over from iteration 1 (non-negotiable)**

- Processors read the **normalized** transaction, never a bank's raw JSON.
- Nothing that a human decided may be overwritten by a replay or a re-run.
- Every Postgres table added here is a **projection**; dropping it and
  replaying the log reproduces it exactly.

---

## 2. Architecture

```
finreport.transaction ──> labeler ──> finreport.transaction-label ─┐
          (normalized via the WP0 mapper registry)                 │
GraphQL mutations ──────> finreport.user-label ────────────────────┼─> projector ──> Postgres
                     └──> finreport.rule ─────────────────────────┤
                     └──> finreport.category ────────────────────-┘
```

### 2.1 Why user overrides live in Kafka

Iteration 1's principle is that Postgres is rebuildable from offset 0. If a user
override lived only in Postgres, that stops being true: a rebuild would either
wipe the user's decisions or need a carve-out growing with every human-owned
table. So **overrides, splits, rules and the category tree are all published to
compacted topics and projected like everything else** — compacted on the
entity's own key, so the log stays the size of the state, not of the edit
history.

**Read-your-write.** A mutation does two things in one request: it publishes the
event (awaiting the broker ack — a publish failure fails the mutation) **and**
applies the identical upsert to the projection. Both carry the same `revision`
(the mutation's RFC 3339 clock, in the payload), and every projection upsert for
these topics is guarded by `WHERE excluded.revision >= row.revision`, so the
projector re-applying the record later is a no-op and the two paths converge.

**Rejected: overrides in Postgres only.** Simpler today, but it reintroduces a
second source of truth one iteration after removing the first, and it makes
"rebuild the read model" a destructive operation nobody dares run.

### 2.2 New topics

| Topic | Key | Cleanup | Payload |
|---|---|---|---|
| `finreport.transaction-label` | `<source>:<external_id>` | compact | labeler output (§2.4) |
| `finreport.user-label` | `<source>:<external_id>` | compact | override + splits (§2.6) |
| `finreport.rule` | rule UUID | compact | rule state (§2.7) |
| `finreport.category` | category UUID | compact | category node (§3) |

All four are **ours**, not a bank's — so unlike the ingest topics they carry our
own JSON, versioned by a `schema_version` field in the payload as well as the
header. The iteration-1 envelope headers still apply; `origin` is `labeler` for
label records and `user` for the three human-edited topics. A tombstone (null
value) means "this entity no longer exists" and projects to a delete.
Partitions 1, RF 1, `prevent_destroy`, as the ingest topics. Keys are the
transaction's `(source, external_id)` identity, not the projected UUID, so the
topics are readable with `rpk` without a Postgres lookup and survive a rebuild
that has not run yet.

### 2.3 The labeler

New binary `webapp/src/bin/labeler.rs` (`[[bin]] name = "labeler"`), modelled
on `projector.rs`:

- Consumes `finreport.transaction` only. No consumer group; offsets in
  `projection_offset` under topic key `finreport.transaction@labeler` (a
  distinct row from the projector's, same table, same transactional commit).
- Each record is normalized through the **existing**
  `projection::mapper::MapperRegistry` into a `TransactionRecord`. The labeler
  contains no per-source code and never touches `raw_payload`.
- Resolution (§2.5) reads rules, overrides, the LLM cache and the category tree
  from the **projection** (they are projections of Kafka; see the lag note
  below), then publishes a label record — **only if it differs from the label
  already on record for that key**. That compare-before-publish is what makes a
  replay from offset 0 produce zero new records and zero LLM calls.
- Batch: same 500-record / 500 ms window, same retry/exit policy and the same
  poison-record skip as the projector.
- `--until-caught-up` exits at the end of the log (used by `dev-demo` and the
  integration tests), otherwise it tails.
- `--reapply <rule-id|--all>` re-resolves the transactions a rule affects
  without consuming new records: the retroactive rule application of
  `docs/requirements.md`. Idempotent, and it skips any transaction with a user
  override or a split.

**Projection lag is accepted.** The labeler may briefly see a rule before it is
active in the projection; the label then resolves to the next source down and is
corrected on the next `--reapply` or the next time the transaction is seen. No
data is lost, and compare-before-publish keeps the correction to one record.

**Cost guard.** `APP_llm_max_requests_per_run` (default 200) bounds LLM calls
per process run. On exhaustion the labeler warns and leaves the rest
**unlabelled** (not held, not failed); the next run picks them up. An unbounded
first run over years of history is the one way this iteration costs real money
by accident.

### 2.4 Label record

The payload mirrors the `transaction_label` columns of §3, plus the transaction
identity (`source`, `external_id`) and `schema_version`, with one difference:
categories are referenced by **slug**, not UUID, so the log stays readable and a
rebuild resolves slugs through the category tree it has just built.
`category_slug` is null unless `status = resolved`; `confidence`, `provider`,
`model`, `prompt_version`, `fingerprint` and `reasoning` are null unless the
label came from the LLM or its cache; `proposed_category_path` carries an LLM
suggestion that is **never** auto-created.

### 2.5 Resolution chain

Exactly `docs/architecture.md` §2:

1. **Split** — a split transaction has no single category; its parts carry
   theirs. The labeler publishes `status=resolved, label_source=user,
   category_slug=null` and stops. (The split itself lives on `user-label`.)
2. **User override** → `label_source=user`, confidence `null`.
3. **Rule** — the most specific *active* rule whose conditions all match
   (§2.7) → `label_source=rule`, `rule_id` set.
4. **LLM cache** — exact `fingerprint` hit → `label_source=llm-cache`, with the
   cached confidence, provider, model and prompt version copied through.
5. **LLM** — one provider call (§2.8). An answer naming a known category slug
   resolves; an answer naming an unknown one becomes
   `status=needs_review, review_reason=new_category`; an answer the provider
   flags as ambiguous, or below `APP_llm_min_confidence` (default 0.5), becomes
   `review_reason=ambiguous`.

**Fingerprint** = `sha256(provider_id ‖ model ‖ prompt_version ‖
normalized_counterparty ‖ normalized_description ‖ direction)`. Amount is
deliberately **excluded**: the same merchant at a different price is the same
answer, and including it would miss nearly every cache hit. Changing provider,
model or prompt version changes the key, so old entries are neither reused nor
deleted.

**Normalization** (`labeling::normalize`, pure, unit-tested): lowercase, strip
accents, collapse whitespace, drop a trailing legal form (`gmbh`, `ag`, `e.k.`,
`kg`, `se`), drop card-terminal noise (`//`, `sagt danke`, trailing store
numbers, `kartenzahlung`, a trailing date), falling back to the normalized
description when the counterparty is empty. The projector writes the result to
the new `transaction.counterparty_key` column (§3) so rules and the learner can
index on it.

**Edge cases**

- A provider error or timeout publishes **nothing**: the transaction stays
  unlabelled and is retried next run. Held and unlabelled are different states
  and must not be conflated.
- A transaction whose category was archived keeps its label; the category just
  stops being offered.
- A zero-amount transaction is labelled like any other; `kind` decides where it
  counts, and zero counts nowhere.
- `NOTBOOKED` transactions are labelled and re-resolved when the booked version
  arrives (same `external_id` ⇒ same key ⇒ compare-before-publish decides).

### 2.6 User label record (overrides + splits)

```jsonc
{
  "schema_version": 1, "source": "...", "external_id": "...",
  "category_slug": "food.restaurants|null",   // null when parts are present
  "parts": [ { "index": 0, "amount": "-12.3400", "category_slug": "food.groceries" } ],
  "revision": "RFC3339",                      // §2.1 last-writer-wins
  "note": "string|null"
}
```

Split validation (mutation-side, and re-checked by the projector):

- ≥ 2 parts; every part amount non-zero and the **same sign** as the
  transaction; parts sum **exactly** to the transaction amount at
  `NUMERIC(20,4)` — no tolerance, no rounding.
- A zero-amount transaction cannot be split.
- Parts inherit the transaction's currency and dates. A part cannot itself be
  split.
- Clearing an override (`category_slug: null, parts: []`) is a real event, not
  a tombstone: it means "fall back to the next source", and the labeler
  re-resolves. A tombstone means the transaction is gone.
- Totals use parts instead of the whole transaction whenever parts exist
  (§5 `categoryBreakdown`, `cashflowSummary` is unaffected — it is amount-only).

### 2.7 Rules

The payload mirrors the `rule` columns of §3 (by `category_slug`, not id), plus
`schema_version`. The only shape defined nowhere else is `conditions` — all
present conditions AND together:

```jsonc
{
  "counterparty_key": "lidl",                     // normalized, exact
  "counterparty_iban": "DE..",
  "description_regex": "…", "description_contains": "…",
  "direction": "SPENDING", "amount_min": "0", "amount_max": "50",
  "account_ids": ["uuid"]
}
```

**Most specific wins**: order by (a) explicit `priority` descending, then
(b) a specificity score = number of conditions present, with
`counterparty_iban` and `counterparty_key` weighted 2 and a bounded amount
range weighted 1, then (c) `id` ascending. (c) exists only so the outcome is
deterministic — never let a tie be decided by row order.

`description_regex` is compiled with a size limit and rejected at the mutation
if it does not compile; an invalid regex that reaches the labeler anyway is
logged and the rule skipped, never a panic.

### 2.8 Rule learning

Per `counterparty_key`, over labels whose source is `user`, `llm` or
`llm-cache` (not `rule` — a rule's own output must not justify itself):

- **N consecutive consistent labels** (`APP_rule_learn_min_observations`,
  default 3) for the same category ⇒ candidate.
- **Confidence** = min of the underlying confidences; a user-confirmed label
  counts as `1.0`. Three user confirmations therefore give `1.0`.
- **Any conflicting category** for that key disqualifies the candidate
  (`state` is never written; the learner records nothing). A later user
  override that makes the history consistent again re-opens it.
- `confidence >= APP_rule_auto_approve_threshold` (default `0.9`, **inclusive**
  at the boundary) ⇒ published as `state=active, origin=learned,
  auto_approved=true`. Below ⇒ `state=in_review`, surfaced in the review queue.
- **Revoking** a rule publishes `state=revoked` and triggers
  `labeler --reapply <id>`; every label it set falls to the next source. A
  revoked rule is never re-learned from the same evidence — the learner skips a
  `counterparty_key` that has a revoked rule for the same category.
- The learner runs inside the labeler process, after each batch, over the
  projection. It is idempotent: a candidate already published with the same
  category and confidence is not republished.

### 2.9 LLM providers

```rust
// categorizer/src/provider/mod.rs
pub struct LabelRequest<'a> {
    pub counterparty: Option<&'a str>, pub description: Option<&'a str>,
    pub amount: Decimal, pub currency: &'a str, pub booking_date: NaiveDate,
    pub transaction_type: Option<&'a str>,
    pub catalog: &'a CategoryCatalog,   // slug + name + kind, ≤3 levels
    pub prompt_version: &'a str,
}

pub struct LabelSuggestion {
    pub category_slug: Option<String>,  // None ⇒ the model declined
    pub proposed_path: Option<String>,  // a category it wants created
    pub confidence: f32,                // clamped to 0.0..=1.0
    pub ambiguous: bool,
    pub reasoning: Option<String>,
}

#[async_trait]
pub trait LabelProvider: Send + Sync {
    fn id(&self) -> &'static str;       // "anthropic"|"ollama"|"openai"|"fake"
    fn model(&self) -> &str;
    async fn suggest(&self, req: &LabelRequest<'_>) -> Result<LabelSuggestion, ProviderError>;
}
```

- Selection by `APP_llm_provider`; **default `fake`**, so nothing ever calls a
  paid API without being told to.
- `anthropic`: Messages API, `APP_anthropic_api_key` as `SecretString`.
- `ollama`: `APP_llm_base_url` (default `http://localhost:11434`), no key.
- `openai`: any OpenAI-compatible `/v1/chat/completions` at `APP_llm_base_url`,
  optional `APP_llm_api_key` — covers llama.cpp, vLLM, LM Studio, Unsloth.
- `fake`: **deterministic** — a committed keyword table maps
  `counterparty_key` → slug with a fixed confidence; unknown keys hash to a
  stable slug at confidence `0.42` (below the review threshold). Two fixture
  counterparties are wired to return `ambiguous` and a `proposed_path`, so the
  local demo and the tests exercise both review reasons with no API key, no GPU
  and no network. Same input ⇒ same output, forever.
- All real providers: one jittered retry on 429/5xx, `APP_llm_timeout_ms`
  (default 20000), structured-JSON parsing, a parse failure mapped to
  `ProviderError` (never to a label).
- The prompt lives in `prompts/categorize.txt`, rewritten for the slug catalog
  and the `confidence` / `ambiguous` / `proposed_path` response fields.
  `APP_prompt_version` is part of the fingerprint; bumping it invalidates the
  cache on purpose.

The `categorizer` crate is rewritten around this trait: its ad-hoc
`Settings`/`dotenv`/`env_logger` loading goes (settings come from
`utils::settings`) and `tracing` replaces `log`.

---

## 3. Postgres schema

sea-orm migrations in `finreport-rs/migration/src/`, then
`make migrate && make generate-entities`. Money is `NUMERIC(20,4)`, confidences
`NUMERIC(4,3)`. Every table here is a projection (§2.1).

**`m2026…_categories`**

```
category(id UUID PK,                      -- UUIDv5(FINREPORT_NS, "category\0"+slug)
         slug TEXT NOT NULL UNIQUE,       -- dotted path, e.g. "food.groceries"
         parent_id UUID NULL -> category.id, name TEXT NOT NULL,
         kind TEXT NOT NULL,              -- income|expense|transfer|saving
         depth SMALLINT NOT NULL CHECK (depth BETWEEN 1 AND 3),
         sort_order INT NOT NULL DEFAULT 0, archived BOOL NOT NULL DEFAULT false,
         origin TEXT NOT NULL,            -- 'seed'|'user'
         owner_user_id UUID NULL -> app_user.id,   -- reserved; always NULL here
         revision TIMESTAMPTZ NOT NULL)
```
`slug` is the identity the event log uses and the LLM cache is keyed against, so
it never changes; renaming edits `name` only. `owner_user_id` is the per-tenant
seam `docs/requirements.md` asks for, added now because adding a column to a
projection later means a rebuild. A child's `kind` must equal its parent's
(checked in the mutation).

**`m2026…_labels`**

```
transaction_label(transaction_id UUID PK -> transaction.id ON DELETE CASCADE,
                  category_id UUID NULL -> category.id,
                  label_source TEXT NOT NULL,       -- user|rule|llm-cache|llm
                  rule_id UUID NULL -> rule.id, confidence NUMERIC(4,3) NULL,
                  status TEXT NOT NULL,             -- resolved|needs_review
                  review_reason TEXT NULL, proposed_category_path TEXT NULL,
                  provider TEXT NULL, model TEXT NULL, prompt_version TEXT NULL,
                  fingerprint TEXT NULL, reasoning TEXT NULL,
                  labeled_at TIMESTAMPTZ NOT NULL)
-- indexes: (status) for the review queue, (category_id) for the breakdown

transaction_user_label(transaction_id UUID PK -> transaction.id ON DELETE CASCADE,
                       category_id UUID NULL -> category.id,
                       note TEXT NULL, revision TIMESTAMPTZ NOT NULL)

transaction_split(id UUID PK,                       -- UUIDv5(txn_id, part_index)
                  transaction_id UUID NOT NULL -> transaction.id ON DELETE CASCADE,
                  part_index INT NOT NULL, amount NUMERIC(20,4) NOT NULL,
                  category_id UUID NOT NULL -> category.id,
                  UNIQUE (transaction_id, part_index))

llm_label_cache(fingerprint TEXT PK, category_id UUID NULL -> category.id,
                proposed_path TEXT NULL, confidence NUMERIC(4,3) NOT NULL,
                provider TEXT NOT NULL, model TEXT NOT NULL,
                prompt_version TEXT NOT NULL, reasoning TEXT NULL,
                created_at TIMESTAMPTZ NOT NULL)
```
`llm_label_cache` is **derived from `finreport.transaction-label`** (every
record with `label_source = llm`), not its own topic — the answers are already
in the log, and a second copy would be a second truth. A rebuild replays them
and the cache comes back without a single provider call.

**`m2026…_rules`**

```
rule(id UUID PK, name TEXT NOT NULL, category_id UUID NOT NULL -> category.id,
     conditions JSONB NOT NULL, priority INT NOT NULL DEFAULT 0,
     state TEXT NOT NULL,                   -- active|in_review|revoked|rejected
     origin TEXT NOT NULL,                  -- user|learned
     auto_approved BOOL NOT NULL DEFAULT false,
     confidence NUMERIC(4,3) NULL, evidence JSONB NULL,
     created_at TIMESTAMPTZ NOT NULL, revision TIMESTAMPTZ NOT NULL)
```
Index on `(state)`. Learned-rule ids are `UUIDv5(FINREPORT_NS,
"rule\0"+counterparty_key+"\0"+category_slug)` — deterministic, so the learner
cannot create two rules for the same evidence across a restart or a replay.

**`m2026…_counterparty_key`**

```
ALTER TABLE transaction ADD COLUMN counterparty_key TEXT NULL;
CREATE INDEX idx_transaction_counterparty_key ON transaction (counterparty_key);
```
Filled by the projector (§2.5 normalization) — a *derived* projector-owned
column, which iteration 1 §1 explicitly allows. Existing rows get it by
replaying the read model (`just dev-reset` locally, the documented rebuild
deployed); no backfill script.

**Seed taxonomy.** `prompts/categories.json` becomes `prompts/taxonomy.json`:
a tree of `{slug, name, kind, children}` derived from today's 14 categories,
with `kind` assigned, the German items kept (`utilities.rundfunk`,
`transportation.kfz_steuer`) and `income` and `transfer` branches added (today's
file is expense-only). The `category-seed` bin publishes one
`finreport.category` record per node; it is idempotent (same slugs ⇒ same ids)
and never deletes a user-created category.

---

## 4. Configuration

New `utils::settings` keys, all optional with the defaults shown:

| Key | Default | Notes |
|---|---|---|
| `APP_llm_provider` | `fake` | `fake`\|`anthropic`\|`ollama`\|`openai` |
| `APP_anthropic_api_key` | — | `SecretString`; required iff provider is `anthropic` |
| `APP_llm_api_key` | — | `SecretString`; optional for `openai` |
| `APP_llm_base_url` | per provider | Ollama / OpenAI-compatible endpoint |
| `APP_llm_model` | per provider | e.g. `claude-sonnet-4-5`, `llama3.1` |
| `APP_llm_timeout_ms` | `20000` | |
| `APP_llm_min_confidence` | `0.5` | below ⇒ review queue |
| `APP_llm_max_requests_per_run` | `200` | cost guard (§2.3) |
| `APP_prompt_version` | `2` | part of the cache fingerprint |
| `APP_rule_learn_min_observations` | `3` | |
| `APP_rule_auto_approve_threshold` | `0.9` | inclusive |

Validation is **lazy, at the provider factory**: `webapp` and the projector
never build a provider, so a missing key must not break their startup.

**Anthropic key wiring.** `.env.tpl` gains exactly one line,
`TF_VAR_anthropic_api_key="op://HomeLab/finreport/anthropic/api_key"` — a single
`op://` reference, nothing shell-expanded, per the `.env.tpl` rule in
`CLAUDE.md`. `terraform/variables.tf` declares it `sensitive = true`;
`docker-compose.yml` passes it to the new `finreport-be-labeler` service as
`APP_anthropic_api_key`. Non-secret knobs (provider, model, threshold) live in
`docker-compose.yml`, never in `.env.tpl`.

---

## 5. GraphQL

Additive only — no iteration-1 field changes, so the frontend packages can land
independently. Frozen by WP0 in `finreport-rs/webapp/schema.graphql` and
mirrored to `finreport-fe/src/lib/graphql/schema.graphql`; the existing
normalized-AST drift test covers it.

```graphql
enum CategoryKind { INCOME, EXPENSE, TRANSFER, SAVING }
enum LabelSource { USER, RULE, LLM_CACHE, LLM }
enum LabelStatus { RESOLVED, NEEDS_REVIEW }
enum ReviewReason { AMBIGUOUS, NEW_CATEGORY }
enum RuleState { ACTIVE, IN_REVIEW, REVOKED, REJECTED }
enum RuleOrigin { USER, LEARNED }

type Category {
  id: UUID!, slug: String!, name: String!, kind: CategoryKind!
  parentId: UUID, depth: Int!, archived: Boolean!, origin: String!
}
type TransactionLabel {
  category: Category                  # null while needsReview
  source: LabelSource!, rule: Rule, confidence: Float
  status: LabelStatus!, reviewReason: ReviewReason
  proposedCategoryPath: String, reasoning: String
}
type TransactionSplit { index: Int!, amount: Decimal!, category: Category! }
extend type Transaction {
  label: TransactionLabel             # null = not labelled yet (≠ needs review)
  splits: [TransactionSplit!]!        # empty when not split
}
type Rule {
  id: UUID!, name: String!, category: Category!, conditions: JSON!
  priority: Int!, state: RuleState!, origin: RuleOrigin!
  autoApproved: Boolean!, confidence: Float, evidenceCount: Int!
  createdAt: DateTime!
}
type CategoryBreakdownRow {
  category: Category!                 # the roll-up level that was requested
  amount: Decimal!                    # positive magnitude
  transactionCount: Int!
  share: Float!                       # of the row's kind total, 0..1
}
type CategoryBreakdown {
  rows: [CategoryBreakdownRow!]!
  uncategorized: CategoryBreakdownRow  # label missing entirely
  needsReview: CategoryBreakdownRow    # held; counted separately, never as spend
  currency: String!
}
type ReviewQueue {
  transactions: [Transaction!]!       # status = NEEDS_REVIEW
  pendingRules: [Rule!]!              # state = IN_REVIEW
  totalCount: Int!
}
extend type Query {
  categories(includeArchived: Boolean! = false): [Category!]!
  categoryBreakdown(filter: TransactionFilter!, level: Int! = 1, kind: CategoryKind): CategoryBreakdown!
  rules(state: RuleState): [Rule!]!
  recentlyAutoApprovedRules(limit: Int! = 20): [Rule!]!
  reviewQueue(page: PageInput): ReviewQueue!
}
extend type Mutation {
  setTransactionCategory(transactionId: UUID!, categorySlug: String!): Transaction!
  clearTransactionCategory(transactionId: UUID!): Transaction!
  splitTransaction(transactionId: UUID!, parts: [SplitPartInput!]!): Transaction!
  unsplitTransaction(transactionId: UUID!): Transaction!
  createCategory(input: CategoryInput!): Category!
  renameCategory(id: UUID!, name: String!): Category!
  archiveCategory(id: UUID!): Category!
  createRule(input: RuleInput!): Rule!
  updateRule(id: UUID!, input: RuleInput!): Rule!
  setRuleState(id: UUID!, state: RuleState!): Rule!
  reapplyRule(id: UUID!): Int!        # transactions re-queued
}
input SplitPartInput { amount: Decimal!, categorySlug: String! }
input CategoryInput { slug: String!, name: String!, kind: CategoryKind!, parentSlug: String }
input RuleInput { name: String!, categorySlug: String!, conditions: JSON!, priority: Int! = 0 }
extend input TransactionFilter {
  categorySlugs: [String!]            # OR-ed; includes descendants
  uncategorized: Boolean              # true ⇒ no label at all
  needsReview: Boolean
  labelSources: [LabelSource!]
}
```

Semantics and edge cases:

- `categoryBreakdown` **uses split parts when a transaction is split**, and the
  whole transaction otherwise. Each transaction is counted once.
- `level` rolls descendants up to that depth (`level: 1` = top-level). `kind`
  null returns every kind, each row's `share` relative to its own kind total —
  so transfers never dilute a spending percentage.
- `transfer`-kind rows are returned but **never counted as spending**;
  `saving`-kind rows count as saving, not spending (`docs/requirements.md`).
- `needsReview` is reported separately and is **not** folded into
  `uncategorized`: held is a decision pending, not an absence.
- `CashflowDimension.CATEGORY` becomes accepted in `cashflowGraph`
  (`[INCOME_SOURCE, ACCOUNT, CATEGORY]`), using the same top-N + `Other`
  folding; `CashflowNode.refType = "category"`, `refId` = the slug, so clicking
  a node drills down through `categorySlugs`. `TAG` stays rejected.
  Flow conservation still holds per account: a split contributes its parts, an
  unlabelled transaction contributes to an `Uncategorized` node rather than
  vanishing (which would silently break reconciliation with `cashflowSummary`).
- Every mutation is scoped: a transaction the caller cannot see is an error, not
  a silent no-op. Mutations publish-then-upsert (§2.1) and return the fresh row.
- `createCategory` rejects depth > 3, a duplicate slug, a slug that does not
  match `^[a-z0-9_]+(\.[a-z0-9_]+){0,2}$`, a parent whose `kind` differs, and a
  parent that is archived.
- `archiveCategory` on a category that still has labels succeeds — history keeps
  rendering it — but it stops appearing in the LLM catalog and in pickers.
  There is no delete.
- `splitTransaction` validation errors carry
  `extensions.code = "SPLIT_SUM_MISMATCH"` with the computed delta.
- `JSON` is a new custom scalar for rule conditions, validated server-side
  against the §2.7 shape; an unknown condition key is a validation error, not
  ignored.

---

## 6. Frontend

Split into two independent areas sharing only the SDL: **analytics** (existing
pages) and **admin** (a new route group).

**Analytics (`/`, `/transactions`)** — a breakdown card on the dashboard
(horizontal bars by top-level category for the period, click → filter); a Sankey
dimension toggle *counterparty* (iteration 1) / *category*
(`[INCOME_SOURCE, ACCOUNT, CATEGORY]`); a category multi-select filter (tree;
selecting a parent includes its children) plus *uncategorized* and *needs
review* toggles; a category column in the transaction table with a **source
badge** (`you` / `rule` / `cached` / `AI`) and a "needs review" pill linking to
the queue. Badges are text + shape, never colour alone (iteration 1 a11y rule).

**Admin (`/admin/*`, new route group, same auth guard)**

- `/admin/review` — held transactions with their reasoning and proposed path:
  *pick a category* / *split* / *create the proposed category and apply*.
  Pending rules show evidence and confidence: *approve* / *edit* / *reject*.
- `/admin/rules` — list with state filter, inline create/edit, an
  **auto-approved** badge, a recently-auto-approved section, revoke, and
  *re-apply* reporting how many transactions were re-queued.
- `/admin/categories` — the tree, create/rename/archive, kind per node, depth-3
  nodes refusing children in the UI as well as the API.
- Split editor — a modal of (amount, category) rows with a live remainder that
  must reach exactly zero before *save* enables, plus *split the remainder*.

Styling rules are unchanged and still binding: **Tailwind 4 utilities only**, no
component library, no `@apply`, no `<style>` blocks, no dark mode, LayerChart the
only charting dependency. Pure helpers (breakdown shaping, split arithmetic in
exact decimal **via strings, never `number`**, which loses cents) live in
`src/lib/` and are what vitest covers; there is still no component-render test
setup.

---

## 7. Local dev

The demo must still run with no API key, no GPU and no network beyond
localhost. That is what `APP_llm_provider=fake` (the default) buys.

- `docker-compose.local.yml` gains the four topics in
  `finreport-redpanda-init` (kept in step with `terraform/kafka` by hand) plus
  an **optional** `ollama` profile, off by default.
- `just` recipes: `dev-labeler` (`cargo run -p webapp --bin labeler --
  --until-caught-up`), `seed-categories` (the `category-seed` bin, idempotent),
  and `dev-demo` extended to `… → projector → seed-categories → labeler
  --until-caught-up →` print next steps. `dev-reset` also truncates the new
  projections.
- Fixtures grow counterparties that exercise the whole chain under the fake
  provider: a repeated merchant that reaches the learning threshold, one
  ambiguous case, one new-category proposal, one already overridden, one split.
- Deploy: a new `finreport-be-labeler` service in `docker-compose.yml`
  (prebuilt image, no new port, no new address), plus the four topics in
  `terraform/kafka` with `prevent_destroy`.

---

## 8. Testing

| Area | Level | What |
|---|---|---|
| Normalization | unit | legal forms, card noise, accents, empty counterparty → description fallback |
| Fingerprint | unit | stable across amounts; changes with provider/model/prompt version |
| Resolution chain | unit | each precedence step wins over the ones below; split short-circuits; clearing an override falls back |
| Rule matching | unit | AND of conditions, specificity ordering, deterministic tie-break, invalid regex skipped not panicking |
| Learner | unit | N-1 vs. N observations, conflict disqualifies, min-confidence, user-confirmed = 1.0, threshold boundary at exactly 0.9, revoked rule not re-learned |
| Splits | unit | exact sum, sign mismatch, single part, zero-amount transaction, 4-decimal remainder |
| Providers | unit | fake determinism; each real provider's response parsing + error mapping against recorded JSON (no network) |
| Breakdown | unit | roll-up by level, splits counted once, transfers excluded from spending, share per kind |
| Labeler | integration | fixture replay → labels; replay again → **zero new records, zero provider calls**; rule revoke → re-apply falls back; cost guard stops at the limit |
| Projection | integration | all four topics project; tombstones delete; out-of-order `revision` does not clobber a newer row; rebuild reproduces identical tables incl. the LLM cache |
| GraphQL | integration | breakdown totals reconcile with `cashflowSummary`; split sum rejection; cross-user mutation denied; `CATEGORY` Sankey conserves flow |
| FE logic | vitest | breakdown shaping, split remainder arithmetic, category-tree selection incl. descendants |
| FE smoke | Playwright | dashboard shows a breakdown; review queue resolves one held transaction and it disappears from the queue |

Integration tests stay behind the `integration` feature and `just
test-integration`.

---

## 9. Dev velocity (adopted as iteration-2 rules)

1. **Shared `CARGO_TARGET_DIR` across worktrees.** Every worktree builds into
   one directory (`finreport-worktrees/.cargo-target`, exported by the
   `justfile` and documented in `CLAUDE.md`), so N parallel agents share one
   dependency build instead of each paying a cold compile. Cargo locks the
   directory, so concurrent builds serialize rather than corrupt.
2. **Migrations run once per test process.** The integration harness wraps
   `run_migrations` in a `tokio::sync::OnceCell` per container, so parallel
   tests in one binary stop racing `sea-orm-migration`'s lock table. This is the
   current flakiness; fixing it is a WP0 deliverable, not a nice-to-have. After
   the fix, integration tests run without `--test-threads=1`.
3. **Integration tests run at the end of a work package, not during.** Unit
   tests are the inner loop. A WP runs `just test` continuously and
   `just test-integration` once, before declaring done.

---

## 10. Work packages

Eight packages. **WP0 freezes every shared contract first**; afterwards each
package writes only files it owns. Each is developed in its own worktree
(`git worktree add ../finreport-worktrees/<branch> -b <branch>`).

### Shared-file protocol

Pre-populated by WP0, edited by nobody else afterwards:
`finreport-rs/webapp/src/lib.rs` (declares `pub mod labeling;`),
`finreport-rs/webapp/Cargo.toml` (all deps + the `labeler`/`category-seed` bin
entries), `finreport-rs/migration/src/lib.rs`,
`finreport-rs/utils/src/settings.rs`,
`finreport-rs/webapp/src/projection/mod.rs` (topic → handler dispatch for the
four new topics, pointing at WP3's module), `finreport-rs/webapp/schema.graphql`
+ its frontend mirror.

### WP0 — Contracts, schema, taxonomy, fixtures, mocks — **S**
**Owns:** `finreport-rs/migration/**`, `finreport-rs/entity/**`,
`finreport-rs/webapp/src/kafka/labeling.rs`, `webapp/src/labeling/mod.rs`
(stubs), `categorizer/src/provider/{mod.rs,fake.rs}` (trait + deterministic
fake), `prompts/taxonomy.json`, `finreport-rs/webapp/fixtures/**`,
`finreport-rs/webapp/schema.graphql`,
`finreport-fe/src/lib/graphql/{schema.graphql,mocks/**}`,
`finreport-rs/webapp/tests/support/migrate.rs` (the §9.2 `OnceCell` fix), plus
the shared files above.

Migrations §3 + regenerated entities; the new topic constants, event structs and
their serde; the §2.9 provider trait and the fake; the taxonomy; fixtures
covering every §7 case; GraphQL mocks for breakdown, review queue, rules and a
split transaction; all settings keys; the migration-race fix.
*Depends on:* nothing. *Blocks:* everything. Keep it thin.
*Done when:* `make migrate` is clean, entities regenerate unchanged, the
workspace builds, `schema.graphql` parses, mocks validate against it, and
`just test-integration` passes **without** `--test-threads=1`.

### WP1 — LLM providers — **M**
**Owns:** `finreport-rs/categorizer/**` except WP0's `provider/{mod.rs,fake.rs}`
(read-only), `prompts/categorize.txt`.
Anthropic, Ollama and OpenAI-compatible implementations; prompt rewrite for the
slug catalog and the structured response; retry/timeout/parse-error mapping;
drop `dotenv`/`env_logger`/`log` in favour of `utils::settings` + `tracing`.
*Depends on:* WP0. *Done when:* each provider parses a recorded response, maps
429/5xx/timeout/garbage to `ProviderError`, and no provider is constructible
without its required config.

### WP2 — Resolution chain, rules engine, learner — **M**
**Owns:** `finreport-rs/webapp/src/labeling/{normalize.rs,fingerprint.rs,
resolve.rs,rules.rs,learn.rs}`.
Pure-as-possible library: normalization, fingerprinting, precedence, rule
matching + specificity ordering, learner with confidence/conflict/threshold
logic. No Kafka, no actix — so WP3 and WP4 can call it immediately.
*Depends on:* WP0. *Parallel with:* WP1, WP3–WP7.
*Done when:* the §8 unit rows for normalization, fingerprint, resolution, rule
matching and learning pass, including the exactly-0.9 boundary.

### WP3 — Labeler processor & new projections — **M**
**Owns:** `finreport-rs/webapp/src/labeling/processor.rs`,
`finreport-rs/webapp/src/bin/{labeler.rs,category_seed.rs}`,
`finreport-rs/webapp/src/projection/labeling.rs`,
`finreport-rs/webapp/tests/labeler_*.rs`.
The consume → normalize → resolve → compare → publish loop with its own offset
rows, `--until-caught-up` and `--reapply`; the cost guard; the projections for
all four new topics including tombstones, the `revision` guard and the
cache-from-labels derivation; the idempotent category seeder.
*Depends on:* WP0, WP2 (signatures fixed in §2.5/§2.8 — stub locally if needed).
*Done when:* a fixture replay labels every transaction, a second replay
publishes nothing and calls no provider, a revoke+reapply falls back, and a
rebuild reproduces identical tables including `llm_label_cache`.

### WP4 — GraphQL — **M**
**Owns:** `finreport-rs/webapp/src/graphql/**`.
Category/label/rule/split types, `categoryBreakdown`, `reviewQueue`, the new
filters, the `CATEGORY` Sankey dimension, every §5 mutation with its
publish-then-upsert and its validation (split sum, slug shape, depth, kind
inheritance, regex compile).
*Depends on:* WP0 (SDL, entities), WP2 (validation helpers).
*Done when:* the SDL drift test passes, breakdown reconciles with
`cashflowSummary`, splits are counted once, and cross-user mutations are denied.

### WP5 — Frontend A: analytics — **M**
**Owns:** `finreport-fe/src/routes/(app)/+page.*`,
`finreport-fe/src/routes/(app)/transactions/**`,
`finreport-fe/src/lib/components/{CategoryBreakdown,CategoryFilter,CategoryPicker,
Badge,Tree}.svelte`, `finreport-fe/src/lib/categoryTree.ts` +
`breakdownShaping.ts` and their tests.
Breakdown card, Sankey dimension toggle, category/uncategorized/needs-review
filters, label-source badges in the table.
*Depends on:* WP0's SDL + mocks only; never blocked on a backend
(`PUBLIC_USE_MOCKS=1`). *Parallel with:* WP6 — disjoint routes, and the only
shared files (`CategoryPicker`, `Badge`, `Tree`) are owned here and consumed
read-only by WP6.
*Done when:* `npm run check`/`lint` pass, vitest covers the shaping helpers, the
pages render fully from mocks.

### WP6 — Frontend B: admin — **M**
**Owns:** `finreport-fe/src/routes/(app)/admin/**`,
`finreport-fe/src/lib/components/{Modal,SplitEditor,RuleForm,ReviewCard}.svelte`,
`finreport-fe/src/lib/splitMath.ts` + its tests,
`finreport-fe/src/lib/graphql/adminQueries.ts`.
Review queue, rules list (auto-approved badge, recently-auto-approved section,
revoke, re-apply), category tree admin, split editor with its exact-decimal
remainder.
*Depends on:* WP0's SDL + mocks. *Parallel with:* WP5.
*Done when:* every admin screen renders and submits against mocks, split
arithmetic is exact at four decimals in vitest, and the Playwright review-queue
smoke passes.

### WP7 — Dev stack, topics, secrets, docs — **S**
**Owns:** `docker-compose.local.yml`, `docker-compose.yml`, `justfile`,
`terraform/kafka/**`, `terraform/variables.tf`, `.env.tpl`, `.gitea/**`,
root `CLAUDE.md`, `docs/architecture.md`.
The four topics (local init + Terraform, `prevent_destroy`), the optional
`ollama` compose profile, `dev-labeler`/`seed-categories`/extended
`dev-demo`/`dev-reset`, the shared `CARGO_TARGET_DIR` export (§9.1), the
`finreport-be-labeler` service, the `TF_VAR_anthropic_api_key` wiring (§4), and
the `CLAUDE.md`/`architecture.md` updates describing the labeling pipeline.
*Depends on:* WP0 (bin names and topic constants are fixed there).
*Done when:* a clean checkout reaches a labelled dashboard and a populated
review queue using only documented commands, with no API key and no GPU.

### Parallelism

```
WP0  ████ (short, blocking)
     ├─> WP1 providers   ─┐
     ├─> WP2 chain+rules  ┤
     ├─> WP3 labeler *    ┤  disjoint file sets
     ├─> WP4 graphql *    ┤  (* soft dep on WP2's signatures, fixed in §2.5/§2.8)
     ├─> WP5 fe analytics ┤
     ├─> WP6 fe admin     ┤
     └─> WP7 dev stack   ─┘
```

---

## 11. Assumptions

1. **Overrides, splits, rules and categories are Kafka-sourced** and
   dual-written to their projection for read-your-write (§2.1). The alternative
   — Postgres-only human state — was rejected because it breaks the rebuild
   guarantee one iteration after it was won.
2. The LLM cache is **derived from the label topic**, not its own topic.
3. Default provider is `fake`. Nothing in the repo calls a paid API unless
   `APP_llm_provider` says so.
4. One category tree, `owner_user_id` reserved but always NULL; per-tenant trees
   arrive with real multi-tenancy.
5. Category identity is the **slug**; renaming never touches it, so the cache
   survives renames. Categories are archived, never deleted.
6. The labeler runs as a single instance, like the projector. Its offsets live
   beside the projector's in `projection_offset` under a distinct topic key.
7. Amount is excluded from the cache fingerprint (§2.5).
8. The learner ignores rule-sourced labels, so rules cannot bootstrap
   themselves.
9. `counterparty_key` is a derived projector column; existing rows acquire it
   through the normal read-model rebuild, not a bespoke backfill.
