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
  - How the category condition and the tag condition combine is **set per goal**: `all` (AND) or `any` (OR). Whatever the goal specifies applies.
    - AND example: "restaurants on vacation" = category Food › Restaurants AND tag `italy-2026`.
    - OR example: "hobby spending" = category Leisure › Hobbies OR tag `hobby`, which also catches hobby purchases filed under other categories.
    - When a goal has both conditions, the UI proposes AND by default and shows the choice explicitly.
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

## Admin user management via UI (iteration 4+)
Scheduled for a future iteration, after the admin bootstrap (`app_user.is_admin`, `webapp::auth::bootstrap`) lands the single automatically-managed `admin` account. Today, creating/disabling additional users and (re)linking accounts is `user-admin`-only (CLI, `webapp/src/bin/user_admin.rs`).

- An `isAdmin` GraphQL session (`Me.isAdmin`) can see an admin-only area to:
  - Create/disable additional users (not just the bootstrap-managed `admin`).
  - Reset a user's password.
  - Assign/unassign account ownership (`user_account` links) per user, equivalent to `user-admin link`/`unlink` but from the UI.
- Out of scope until then: no GraphQL mutations for any of the above exist yet, and the frontend has no admin area — `user-admin` remains the only way to manage users beyond the bootstrap-managed `admin`.

## Bank connections via UI (future iteration)
Replaces the preconfigured Comdirect logins (`APP_accounts__<n>__*` env vars, `.env.tpl`/terraform). Applies to Comdirect and any later bank.

- **Credentials are user-managed in the UI:** each user adds, edits and removes their own bank connections (Comdirect: client id, client secret, Zugangsnummer, PIN). Nothing bank-specific is configured through env, terraform or 1Password any more.
- **Sync (login) is started from the UI**, per connection, not by the importer on a schedule with baked-in creds.
- **TAN state is visible in the UI:** a sync shows its live status (e.g. logging in → *TAN pending — approve the push-TAN in your app* → importing → done/failed). A pending TAN never blocks other connections or users; a TAN that expires or is rejected shows as failed with a retry action.
- **Ownership:** a connection belongs to the user who created it; accounts it imports are linked to that user.
- **Edge cases:**
  - Session tokens are still persisted per connection, so re-syncs within the bank's session lifetime need no new TAN.
  - Two users connecting logins that see the same account: same idempotent import rules as today (dedup by Comdirect `accountId`).
  - Deleting a connection stops future syncs; already-imported data stays.
  - Wrong credentials surface as a connection error in the UI, not a crash-looping importer.
- **Security (must be decided with the user before implementation):** how credentials are stored (encrypted at rest, key management), whether the PIN is stored at all or asked per sync, and who can see/edit a connection (never returned to the browser after saving).
- **Migration:** existing env-configured logins keep working until the UI flow ships; then they are removed from `.env.tpl`, terraform and `docker-compose.yml`.

## More bank/broker integrations (next planned feature)
New sources: **C24 Bank**, **PayPal**, **Scalable Capital**. They plug into the same pipeline as Comdirect: an importer per source publishes raw payloads to its own Kafka topics (payload byte-for-byte, our metadata in headers), the projector maps them into the shared read model, and connections are configured per user via the UI flow above.

- **Per-source adapter:** one crate per source behind a common `Source` trait (login/session, list accounts, fetch balances, fetch transactions newer than a watermark). Comdirect is refactored behind the same trait.
- **Access method is open per source and must be researched before speccing** (official API vs. PSD2/aggregator vs. file import). Fallback for any source without a usable API: CSV/statement upload in the UI, deduplicated like an API import.
- **Scalable Capital is a broker:** besides cash movements it has securities positions and trades. Iteration scope: cash account transactions and portfolio value as a balance; per-security holdings/performance are a separate later feature.
- **PayPal specifics:** a PayPal payment is usually also visible on the funding bank account. Detect and link these pairs (like internal transfers) so spending is not double-counted; the PayPal side carries the real merchant and is the one categorized.
- **Edge cases:** multi-currency PayPal balances (convert or show per currency — decide in spec); pending vs. booked entries; refunds/chargebacks; sources whose transaction ids are not stable across exports (dedup key must be defined per source).
- **Security:** same credential-storage decision as "Bank connections via UI" applies.

## Transaction detail modal (next planned feature)
Clicking any transaction, **from every place one is shown**, opens a detail
modal where its classification can be edited in place: dashboard list,
`/transactions`, `/recurring`, the admin review queue, and the goal pages
(iteration 4). Today editing is scattered — the review queue has its own card,
splits have their own editor, and a transaction in a plain list cannot be
edited at all.

- **Editable in the modal:** category (with the existing `CategoryPicker`),
  split into parts (`SplitEditor`), tags (`TagEditor`), and the recurring
  override (`RecurringBadge`'s you/auto toggle).
- **Shown, not editable:** amount, dates, counterparty, description, booking
  status, the label's source badge (user / rule / llm-cache / llm), the
  transfer badge and its counterpart, and the detected recurring series.
- **Backend work: none expected.** `setTransactionCategory`,
  `splitTransaction`/`unsplitTransaction`, `setTransactionTags` and
  `setTransactionRecurring` already exist and already return the updated
  `Transaction`, including a label that reflects a just-written user decision
  rather than the labeler's trailing projection. The gap is purely in the UI.
- **Two components to extract, which is the point of doing this once:**
  - a `TransactionItem` used by every list, so a transaction looks and behaves
    the same everywhere and the click target lives in one place. Each list
    currently renders rows its own way (`TransactionTable`, `ReviewCard`).
  - the modal itself. `Modal.svelte` **already exists** (`SplitEditor` uses
    it), but `RuleForm.svelte` and `/admin/categories` hand-roll a native
    `<dialog>` and carry stale comments claiming the repo has no `Modal.svelte`.
    Converging all three on the one component is part of this work.
- **Edge cases:**
  - Closing without saving discards nothing silently — either save per field on
    change, or keep an explicit save with a dirty-state guard. Decide in spec.
  - A category change clears splits (existing mutation semantics); the modal
    must say so before doing it, not after.
  - Accessibility: focus trap, Esc to close, restore focus to the clicked row.
  - Mobile width: the modal becomes a full-height sheet rather than a dialog.
  - Opening a transaction held for review should offer the same resolution
    actions as the review queue, so the queue becomes one entry point rather
    than a separate flow.

## Dashboard transaction filters (next planned feature)
The dashboard's transaction list gets the same filter controls `/transactions`
already has, placed next to the list: **account(s), category(ies), tag(s),
amount range, and flags**.

- **Mostly already supported.** `TransactionFilter` accepts `accountIds`,
  `categorySlugs` (OR-ed, descendants included), `tags` (AND-ed), `direction`,
  `search`, and the flags `recurring`, `transfer`, `needsReview`,
  `uncategorized` and `labelSources`. `/transactions` and the admin pages
  already render controls for most of these (`CategoryFilter`, the tag filter,
  the account select).
- **The one backend gap: amount range.** Add `amountMin` / `amountMax`
  (inclusive, either side optional) to `TransactionFilter` and to every
  resolver that takes it, so the dashboard, `/transactions`, the cashflow
  queries and the goal drill-down all honour it identically. Decide in spec
  whether the bound applies to the signed amount or its magnitude — magnitude
  is what a user means by "over €100", but signed is what "income above X"
  needs; the likely answer is magnitude plus the existing `direction`.
- **Open design question: what the filter narrows.** Today the dashboard's
  Sankey/bar drill-down narrows *only the transaction list* and deliberately
  leaves the charts scoped to the whole period. A filter panel next to the
  list could do either. Narrowing the charts too makes the totals agree with
  the list, which is probably what a user expects; keeping them whole
  preserves the "see the period, drill into a slice" behaviour. Pick one
  explicitly and say so in the UI, rather than leaving it ambiguous.
- **Reuse, don't re-add:** extract the filter panel from `/transactions` into
  one component used by both pages, so the two cannot drift. Filter state
  stays in search params, as it does today, so a filtered view is linkable.
- **Edge cases:** an empty result reads as "no transactions match" rather than
  an empty chart; flags are tri-state (on / off / don't care), not checkboxes
  that silently mean "off"; clearing all filters is one action.
