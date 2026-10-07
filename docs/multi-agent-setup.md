# Multi-agent development setup

Running log of what's been done to prep this repo for multiple Claude Code agents working in parallel, and why. Update this file as further prep steps land.

## Roles

Three roles, run in this order for any feature built via this flow:

| Role | When | Model | Job |
|---|---|---|---|
| **tech-designer** | Before any implementation | `opus` | Verify assumptions against the actual codebase, produce the concrete task split + contract (exact API/schema shape, which files/areas each implementer owns). Surface it before implementation starts — this is a checkpoint, not a rubber stamp. |
| **implementer** | Per independent slice of the split | `sonnet` | Build one slice against the contract, worktree-isolated (`isolation: "worktree"`) so parallel implementers can't collide on files. |
| **reviewer** | After integration | `opus` | Review the merged/integrated result, not each slice in isolation. |

Reviewer and tech-designer both run on the stronger model — a bad split poisons everything downstream just as much as a missed bug does, so both get the same tier.

## 2026-07-17

- **Added root `CLAUDE.md`.** Every agent needs the same baseline context (workspace layout, conventions, commands) without re-deriving it from scratch each session. Covers the `finreport-rs` Cargo workspace + `finreport-fe` layout, the `tracing`/`secrecy` conventions, and corrects the stale sqlx-cli note in `README.md` (this project uses sea-orm-cli).
- **Added `finreport-fe/CLAUDE.md`.** Frontend has its own toolchain (npm/SvelteKit) and check/lint/test commands distinct from the Rust workspace; a directory-scoped CLAUDE.md lets an agent working only in `finreport-fe/` get relevant context without pulling in backend detail.
- **Documented the worktree convention** in root `CLAUDE.md`: parallel agents get an isolated working copy via
  ```bash
  git worktree add ../finreport-worktrees/<branch-name> -b <branch-name>
  ```
  Worktrees are kept as **sibling directories** (outside the repo), not nested inside it — avoids polluting `git status`/`.gitignore` inside the main tree and keeps `target/`, `node_modules/`, etc. per-worktree so parallel builds don't fight over the same `finreport-rs/target`. No worktrees created yet — that happens once there's an actual task to split across agents.

- **Made each stack component independently startable in dev mode**, ahead of the first real task split (frontend: display transactions). Previously the only documented path was the full `docker-compose.local.yml` stack (build required, single combined profile). Now:
  - `just db-up` no longer wraps in `op run` — it never consumed anything from `.env.tpl` (Postgres password has its own compose default), so the 1Password dependency was pure friction for local dev.
  - Added `just dev-be`: `cd finreport-rs && cargo run -p webapp --bin webapp`, reading config from `finreport-rs/.env` (gitignored, template at `finreport-rs/.env.example`) via `dotenv()`. Comdirect creds in that file are dummy placeholders — the webapp binary never calls the Comdirect API, only the importer binaries do (those still go through `just import-local`, which pulls real creds from 1Password).
  - Replaced the ad-hoc `dev:remote` npm script (inline `PUBLIC_GRAPHQL_URL=... vite dev`) with a proper Vite-mode profile: `npm run dev:tower` (`--mode tower`) loads `finreport-fe/.env.tower`. Default `npm run dev` stays the "local" profile via the existing code fallback to `localhost:8080`. See `finreport-fe/CLAUDE.md`.
  - Verified end-to-end: Postgres up via `just db-up`, backend via `just dev-be` (migrations ran, `{ hello }` query returned over HTTP), frontend via `npm run dev` (SSR load hit local backend, `error: null`) and `npm run dev:tower` (confirmed via `vite.loadEnv` that `--mode tower` resolves `PUBLIC_GRAPHQL_URL` to the tower IP while default mode has no override).

## 2026-07-17 (later) — first real parallel run: Transactions tab

First actual multi-agent split, following up on the "next task" note above. Orchestration pattern used, for reuse next time:

1. **Orchestrator defines the contract first.** Before spawning anything, read the entity layer to settle the one non-obvious fact both sides needed (`account_transactions` is the live, populated table — `transactions`/`categories`/etc. are dead code with no migration behind them), then wrote a concrete GraphQL contract (exact type/field names, resolver argument semantics, error behavior) into both agent prompts verbatim. This is what let two agents build compatible halves without seeing each other's code — confirms the earlier "sequence or coordinate" note wasn't quite right: parallelizing across the shared schema surface works fine *if* the schema is nailed down up front and handed to both sides as a spec, not left for them to negotiate.
2. **Two `Agent` calls, `isolation: "worktree"`, one message.** Backend agent wired `DatabaseConnection` into the async-graphql context, added a `CurrentUser`/`scoped_account_ids` seam (in-memory only, no DB table — see the earlier decision), `accounts`/`transactions` resolvers, 7 unit tests. Frontend agent built the tab shell, `/transactions` route with date+account filters, and — since the repo had no unit-test runner at all — added a minimal `vitest` setup scoped to pure logic only (no jsdom/component tests), 15 unit tests. Both finished clean and reported zero contract deviations.
3. **Integration gotcha:** both worktrees forked from `main` at the last commit, which did **not** include this session's still-uncommitted dev-tooling changes (justfile recipes, `dev:tower` npm script). Pulled each branch's changed files into the main tree via `git checkout <branch> -- <paths>` (not `git merge`, to avoid an unrequested commit on `main`), then manually re-applied the `dev:tower` line the frontend branch's `package.json` didn't have.
4. **End-to-end verification caught a real bug the unit tests couldn't**: SvelteKit's SSR `load()` fetch wrapper strips response headers by default; urql's `fetchExchange` needs `content-type` to parse the GraphQL response, so every SSR request silently failed with `data: null` even though `cargo test`/`npm run check`/`vitest` were all green and the backend worked fine over plain `curl`. Fixed with `finreport-fe/src/hooks.server.ts` (`filterSerializedResponseHeaders`). Lesson: unit tests + type-checks on both sides passing does not mean the sides work *together* — driving the actual stack end-to-end is what surfaced this, not any static check.
5. **Review**: `/code-review high` — 8 parallel finder-angle agents against the integrated diff, then verified directly by the orchestrator (re-reading the actual code / re-running live queries against the local DB) rather than spawning a verifier agent per candidate, to control cost. 9 findings survived, most-severe: a GraphQL non-null-field-nulls-the-whole-response bug (verified live) that blanks the account dropdown on a stale `accountId`, and a redundant DB double-query that 5 of 8 angles independently converged on.

## 2026-07-17 (later still) — model selection + tech-designer role

- User asked how to check which model an agent ran as — there's no retroactive log; it has to be set explicitly via the `Agent` tool's `model` param at spawn time, or it silently inherits the parent conversation's model. None of the calls in the Transactions-tab run above set it.
- Set the pairing now formalized in the **Roles** table: implementers on `sonnet`, review on `opus`.
- Added a third role, **tech-designer**, running before implementation to verify assumptions and do the task split — formalizing what the orchestrator did ad hoc (step 1 of the Transactions-tab run above). Put on `opus`, same tier as review, since the contract quality gates everything built on top of it. Use `subagent_type: "Plan"` for this role (read/search/bash only, no write access — it designs, it doesn't implement).

## 2026-07-18 — reviewer missed a real issue; added a fourth review angle

User caught something the 8-angle review missed: the backend implementer had added `finreport-rs/webapp/src/user.rs` directly at the crate's `src/` root, when every other domain module in that crate (`db/`, `graphql/`, `institute/`, `service/`) is its own subdirectory. Root cause: the **Conventions** angle only flagged violations it could quote from a CLAUDE.md rule (none existed for module layout), and **Altitude** was scoped narrowly to the auth-seam design question, not general placement — no angle's job was "does this fit how the surrounding code is organized." Full diagnosis in memory (`multi_agent_review_gaps`, session-local Claude memory, not in this repo).

Fixes applied:
- Moved the file to `finreport-rs/webapp/src/graphql/current_user.rs` (its only consumer, `graphql/queries.rs`, and it's tightly coupled to `async_graphql` types anyway). Updated `lib.rs`/`graphql/mod.rs` wiring. All 7 backend unit tests still pass, now under `graphql::current_user::tests::`.
- Ran a supplementary **structural-placement** pass (single `opus` agent, explicitly told to check new files against *observable* sibling conventions rather than written rules) against the rest of the diff. It found one more: `finreport-fe/vitest.config.ts` was a standalone config that hand-rolled the `$lib` alias and dropped the `sveltekit()`/`tailwindcss()` plugins, instead of extending the project's one canonical `vite.config.ts` (every other tool — Playwright, ESLint, Svelte — has exactly one config file, not two). Merged it into `vite.config.ts` via a `test` block (`defineConfig` from `vitest/config`, the standard SvelteKit+Vitest pattern) and deleted the standalone file. Also fixed a related wiring gap it surfaced: `npm test` only ran `test:e2e`, never `test:unit` — now runs both.
- Added "structural placement, checked against observable sibling conventions" as a **standing fifth angle** for future review passes on this repo (see Roles table below) — it's cheap (one agent, narrow scope) and this run proved the other angles don't cover it.

## Roles (updated)

The three-role table above still holds. Adding to the **reviewer** row: always include a placement/organization angle alongside whatever else runs — explicitly instructed to check new files against sibling-directory conventions *even with no CLAUDE.md rule to point at*, since the Conventions angle's quote-a-rule requirement (kept strict on purpose, to avoid flagging pure style preference) will otherwise never catch this class of issue.

## Open questions / next steps

- Worktrees from this run (`worktree-agent-a41aba1b382164e59` branch/backend, `worktree-agent-a7447852934970734` branch/frontend, under `.claude/worktrees/`) are still on disk — clean up once their contents are confirmed merged and no longer needed for reference.
- Known conflict-prone shared surface confirmed in practice: the GraphQL schema. Worked fine this time because the contract was spec'd up front — don't skip that step for the next split.
- Consider a `finreport-rs/CLAUDE.md` if backend-only agents start needing crate-level detail beyond what's in the root file.
- The `CurrentUser` seam currently does "fetch all accounts, then filter in Rust" rather than a SQL-level ownership predicate — flagged by review as something a real auth system will need to replace rather than extend. Worth deciding before multitenancy work actually starts.

## 2026-08-23 — Kafka migration phase 1

Split four ways, contract-first (topics, keys, and the raw-payload rule fixed
up front so the lanes could not diverge):

- **infra agent** (worktree): `terraform/kafka/` root module, `just
  deploy-kafka`, CI step.
- **comdirect-rs agent** (worktree): `Raw<T>` passthrough + early-stop
  pagination, kept strictly additive so it could not collide with concurrent
  edits to the importer.
- **docs agent** (worktree): `docs/kafka-migration.md`.
- **main session**: the compose service, image build deps, and the
  producer/watermark modules plus the importer wiring.

What was worth repeating:

- `docker-compose.yml` was deliberately kept out of agent hands. It is the
  deployed stack file and had already been clobbered once by a wholesale
  rewrite; additive edits by one owner only.
- Handing each agent an explicit "do not touch X, someone else owns it" list
  prevented every collision except one.

What to fix next time:

- **Agents left their work uncommitted.** Branches still pointed at HEAD, so
  `git checkout <branch> -- <path>` silently returned HEAD's content instead of
  their work. Files had to be copied out of the worktree directories by hand.
  Tell agents explicitly to `git add -A && git commit` on their branch.
- **The contract had a gap**: raw passthrough and early-stop were specified as
  separate functions, so neither could serve the dual-write, which needs both
  at once. Worth walking the actual call site through the proposed API before
  handing it out.
