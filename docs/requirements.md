# finreport — requirements checkpoint 1

## Vision
A web UI for exploring personal and family finances: income and spending by period, category, tag and recurring costs, plus savings and goals. Each view shows charts with the matching transactions and totals. Built iteratively.

## Domain
- **Categories:** a tree, at most 3 levels deep.
- **Tags:** free-form. A transaction can have many tags.
- **Multi-tenant from day one:** there's one user today. Users and accounts are many-to-many, e.g. shared family accounts. Access control (RBAC/ABAC) will be decided later.
- **Many importer sources:** Comdirect now, later C24, Scalable, PayPal and others. The data model must not be specific to Comdirect.
- **Internal transfers:** money moving between the user's own accounts is a first-class case and should be detected automatically.

## Architecture
- Importers write to Kafka (Redpanda).
- Processors read from Kafka and enrich the data. Each processor has one narrow job, with 3–5 processors in total and no more.
  - LLM enrichment, with responses cached to save cost.
  - Ambiguous results are held for the user to decide (human-supervised labelling).
  - Internal-transfer detection.
- A Rust GraphQL server serves both the raw and the enriched data.
- A SvelteKit frontend shows charts and tables. It includes an admin section for resolving held transactions and overriding labels chosen by the LLM.
- Data is stored durably in Postgres and Kafka.

## Constraints
- Stack: Rust, Postgres, Kafka/Redpanda, SvelteKit.
- Reuse existing code and infrastructure that works; clean it up where needed.
- Must run locally.

## Assumptions
- In "data importer(s) read data as it arrives", I read "importer" as **processor**.
- User-set labels and resolved held transactions always take priority over LLM labels, and LLM processing never overwrites them.

## Decisions
1. **Source of truth:** importers write only to Kafka. Postgres becomes a read model built from it. Enrichment is idempotent: replaying events gives the same result.
2. **Recurring costs:** detected automatically. The user can override the flag on any transaction, and the UI highlights overridden values against auto-detected ones.
3. **Savings:** investment positions are not counted for now. Transactions that move money into investments are still recognised and labelled as investment savings.
4. **Goals:** postponed, tracked in `docs/TODO.md`.
5. **Authentication:** simple username/password for now. Authelia/OAuth comes later.
6. **Delivery:** MVP-first iterations. Each one should show the user something useful as early as possible.

## LLM providers (iteration 2)
- The categorizer can use several LLM providers, chosen by config:
  - Anthropic (key `APP_anthropic_api_key`, sourced from `TF_VAR_anthropic_api_key`)
  - local Ollama
  - any OpenAI-compatible endpoint set by its URL, which covers Unsloth models served through llama.cpp, vLLM or LM Studio
- The LLM cache key includes the provider, the model and the prompt version.
- For local dev, Ollama is available as an optional profile in Docker Compose.

## Categories (iteration 2)
- The category tree is per tenant, seeded from a default taxonomy, and at most 3 levels deep.
- Each category has a `kind`: `income | expense | transfer | saving`. `kind` drives the totals:
  - transfers are never counted as spending
  - savings and investments count as savings, not spending
- Each transaction gets one category, at any level. The LLM aims for the deepest level.
- Category ids and slugs stay stable, so renaming a category doesn't invalidate the LLM cache.
- The default taxonomy keeps German-specific items, e.g. Rundfunk and Kfz-Steuer.
- **Split transactions:** one transaction can be split into parts, each with its own category and amount (e.g. an Amazon order covering groceries and electronics).
  - The parts must add up exactly to the transaction amount.
  - When a transaction is split, totals use the parts instead of the whole transaction.
  - Splits belong to the user's override layer: replaying or re-running the LLM never overwrites them.
- **LLM-suggested categories:** the LLM can propose a new category. A proposal goes to the admin review queue and is never created automatically. Until it's approved, the transaction stays in the queue.

## Rules engine (iteration 2)
Goal: don't ask the LLM again for repeating patterns, such as Lidl → groceries.
- **Order of precedence:**
  1. the user's override
  2. a matching rule
  3. the LLM cache (an exact fingerprint match)
  4. a new LLM call
  
  Each label records its source (`user | rule:<id> | llm-cache | llm`), so the UI can highlight how it was set.
- **What a rule matches:** conditions on normalized, source-independent fields:
  - normalized counterparty name
  - counterparty IBAN
  - a regex or keyword on the description
  - direction
  - amount range
  - account

  Several conditions are combined with AND.
- **What a rule does:** assigns a category. Later it can also set tags and the recurring flag. Rules have a priority, and the most specific rule wins.
- **Where rules come from:**
  - the user creates or edits them in the admin UI
  - **learned rules**: when the same normalized counterparty has had the same category N times (default 3), the system creates a candidate rule with a **confidence** score:
    - The score comes from the LLM confidences of the underlying labels, taking the minimum, so one weak label lowers it.
    - A label the user confirmed counts as 1.0.
    - Any conflicting label for that counterparty disqualifies the candidate.
    - The rule stores its score and the transactions it was built from.
  - Candidates with confidence **≥ threshold** (default 0.9, configurable) are approved automatically, but stay **visible**:
    - they carry an "auto-approved" badge
    - they appear in an admin list of recently auto-approved rules
    - the user can revoke one, and its labels then go back to the next source in the order of precedence
  - Candidates **below the threshold** go to the admin review queue.
- **Changing a rule** can be applied retroactively, on demand, to transactions without a user override. The operation is idempotent and never touches user overrides.
- **Ambiguous merchants** (Lidl also sells non-food items): a rule sets the default, and the user corrects individual transactions with an override or a split.

## Savings and spending goals (iteration 4, after categories and tags)
A goal is a threshold that the user sets against a **scope** of transactions and checks over time. Each goal has its own browsable page.

- **Types**
  - `spending_limit`: stay at or below the amount. Example: hobbies ≤ €200/month.
  - `saving_target`: reach at least the amount. Only `saving`-kind categories count toward it.
- **Scope:** one or more categories and/or tags.
  - A category includes its subcategories.
  - Categories and tags combine with **AND**: a transaction must match the category condition *and* the tag condition. Example: category Leisure › Travel AND tag `italy-2026`.
  - Within categories, several categories combine with OR. A transaction (or split part) has only one category, so AND would never match anything.
  - Within tags, several tags combine with AND by default (the transaction carries all of them), with an option to switch to `any`.
  - A scope that has only categories, or only tags, uses just that condition.
  - Each transaction is counted once.
  - Split transactions count by their parts.
  - Internal transfers never count.
- **Period**
  - **Recurring** (monthly by default; quarterly and yearly are also possible). Each period is evaluated on its own, with no rollover of unused budget. Example: "how much do I spend on hobbies each month".
  - **Fixed range** (start date to end date, or open-ended): the total adds up over the whole range. Examples: a renovation project, a vacation.
- **Goal page**
  - Recurring goals: bars per period with the threshold as a line, colored by over or under.
  - Fixed-range goals: a cumulative line against the budget line.
  - Totals: spent or saved, remaining, average per period.
  - The matching transactions, with drill-down.
- **Edge cases**
  - The current period is shown as "in progress", not as a failure.
  - Refunds within the scope reduce spending.
  - Transactions held for review are shown separately as "pending" until they're resolved.
