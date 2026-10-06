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
