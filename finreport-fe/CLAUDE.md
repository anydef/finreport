# finreport-fe

SvelteKit frontend for finreport, consuming the GraphQL backend in `../finreport-rs/webapp` through a same-origin proxy.

## Stack

- SvelteKit 2 + Svelte 5, TypeScript
- `@urql` GraphQL client (`src/lib/graphqlClient.ts`), talking only to the same-origin `/api/graphql` proxy — never directly to the backend
- `layerchart` (`^2.5`) for charts — the **only** charting library in this project (bar chart + Sankey); no `chart.js`, no other chart/CSS framework
- Tailwind CSS 4 utilities only — no component framework, no `<style>` blocks in app code (Storybook's pre-existing `src/stories/**` demo scaffolding is the one exception, out of scope for app styling)
- Storybook 9 for component development (`src/stories/`)
- Playwright for e2e tests (`e2e/`)

## Commands

```bash
npm run dev             # local dev server, local profile (talks to backend on localhost:8080)
npm run dev:tower        # local dev server, tower profile (talks to the deployed Unraid backend)
npm run build / preview
npm run check            # svelte-kit sync + svelte-check (type checking)
npm run lint              # prettier --check + eslint
npm run format             # prettier --write
npm run test               # test:unit then test:e2e
npm run test:unit           # vitest — pure logic only, see Conventions
npm run test:e2e            # playwright, dev server on :5174 in mock mode (see below)
npm run storybook          # component dev server
```

## Auth, the GraphQL proxy, and mock mode

- **`src/routes/api/graphql/+server.ts`** is the only thing the browser ever talks GraphQL to. It's a thin POST-only proxy: rejects non-`application/json` requests, forwards `Content-Type`/`Origin`/the session cookie to the real backend, and relays every upstream `Set-Cookie` back via `Headers.getSetCookie()` (never `.get()`, which would merge multiple cookies with commas). SvelteKit's own CSRF check (`csrf.checkOrigin`, on by default) stays on — the proxy adds to it, not instead of it.
- **`src/lib/server/graphqlBackend.ts`** is the one place that knows how to reach the real backend _or_ synthesize mock responses, shared by the proxy route, `hooks.server.ts` (per-request `me`) and the `/login` form action so the three callers can't drift.
- **`PUBLIC_USE_MOCKS=1`** is the "never blocked on a running backend" switch (`$env/dynamic/public`, so it's runtime-settable, not baked into the build): every GraphQL operation the frontend uses is served from the WP0 fixtures in `src/lib/graphql/mocks/*.json` plus a tiny in-memory session stand-in for `me`/`login` (any non-empty username/password succeeds)/`logout` — no network call at all. Mock responses **must** be the full `{ data: ... }` envelope urql expects, not the bare fixture payload.
- **`src/hooks.server.ts`** populates `event.locals.user` on every request via an in-process `forwardGraphql(ME_QUERY)` call (not a self-HTTP round trip).
- **`src/routes/(app)/+layout.server.ts`** is the auth guard: redirects to `/login?redirectTo=...` when `locals.user` is null. Everything under the `(app)` route group (dashboard, `/transactions`) is guarded this way; `/login` is the only public route.
- **`/login`**: a `+page.server.ts` form action validates the fields, calls `forwardGraphql(LOGIN_MUTATION)`, relays `Set-Cookie`s via `relaySetCookies()`, and redirects to a sanitized `redirectTo` (same-origin only, falls back to `/`).

## Routes

- `(app)` route group — auth-guarded, shares a nav layout (Dashboard / Transactions tabs + logout).
  - `/` — the dashboard: period selector (month/week/custom + granularity), totals, a `CashflowBarChart` (income/spending bars) and a `CashflowSankey` (income → account → spending, with `Other`/`NET`/`DEFICIT` nodes), and a paged transaction list. Clicking a bar narrows the list to that bucket's date range (`txStart`/`txEnd` search params); clicking a Sankey node/link narrows it by the `sel*` params (`selAccountIds`/`selCounterparties`/`selHasCounterparty`/`selCategorySlugs`/`selUncategorized`/`selNeedsReview`, via `chartShaping.ts`'s `drilldownFilterForNode`/`drilldownFilterForLink`) — a chart click narrows only the list. The "Spending by category" card expands a row to its subcategories (child `categoryBreakdown` fetched on open, scoped to the category at `level: depth + 1`) and drills down on the row itself, including the Uncategorized and Needs review rows (`breakdownShaping.ts`'s `drilldownForBar`). The shared `TransactionFilters` panel (search params `accountIds`, `categorySlugs`, `tags`, `search`, `amountMin`/`amountMax`, tri-state `recurring`/`transfer`/`needsReview`/`uncategorized`) is separate and narrows the totals, charts, breakdown *and* list; a panel edit resets the chart selection.
  - `/transactions` — a paged transaction list with the same `TransactionFilters` panel (plus a period select), no charts.
- `/login` — public.
- `/api/graphql` — the proxy (see above).

LayerChart's `Chart` component has a server-rendering bug on this Svelte/Node combination (`ReferenceError: Cannot access 'TransformContext' before initialization`), so both chart components are rendered client-only (`{#if browser}` from `$app/environment`, with a plain-text SSR fallback) rather than patched in `node_modules`.

## Conventions

- Run `npm run check` and `npm run lint` before considering frontend work done.
- Vitest (`npm run test:unit`) covers pure, non-Svelte logic extracted into `src/lib/*.ts`:

  - `src/lib/period.ts` — date range presets, default granularity, bucket labeling.
  - `src/lib/chartShaping.ts` — shaping `CashflowSummary`/`CashflowGraph` GraphQL responses into chart-ready datasets, plus the drilldown-filter builders (covers the `DEFICIT`/`NET`/`Other`/unknown-`kind` edge cases using the actual WP0 mock fixtures).
  - `src/lib/format.ts` — amount/date display formatting.
  - `src/lib/transactionFilters.ts` — the filter panel's logic: search params <-> filter, active-filter chips, tri-state flags, amount bounds, layering a chart selection over the panel.

  There's no component-rendering test setup (no `@testing-library/svelte`/jsdom) — component-level coverage comes from Playwright (e2e) and Storybook. Vitest is configured via a `test` block in `vite.config.ts` (not a separate `vitest.config.ts`) so it shares the real `sveltekit()`/`tailwindcss()` plugin setup — including `$lib`/`$app`/`$env` alias resolution — instead of a hand-rolled duplicate that would drift from it.

- `npm run test:e2e` runs against the **dev server**, not a prod build, on port **5174** (not 5173, so it doesn't collide with a `npm run dev` you already have open) with `PUBLIC_USE_MOCKS=1` baked into `playwright.config.ts`'s `webServer.env` — e2e smoke tests never need a running backend.
- GraphQL operation documents live in `src/lib/graphql/queries.ts` (plain strings — `@urql/core` accepts a string or `DocumentNode`, no `gql` tag needed) and must match the operation names `graphqlBackend.ts`'s mock dispatch switches on (`Me`/`Login`/`Logout`/`Accounts`/`Transactions`/`CashflowSummary`/`CashflowGraph`). Hand-written TS types mirroring the frozen `schema.graphql` live in `src/lib/graphql/types.ts`.
- Reusable, Tailwind-only components live in `src/lib/components/` (`Button`, `Card`, `Field`, `TotalsRow`, `PeriodSelector`, `CashflowBarChart`, `CashflowSankey`, `TransactionTable`, `Pagination`). Chart and UI colors share the same `@theme` tokens defined in `src/app.css` (`--color-income`, `--color-spending`, `--color-net`, `--color-account`, `--color-muted`, `--color-brand`) so a chart's color and its legend/table text never drift apart.
- `src/lib/graphql/schema.graphql` and `src/lib/graphql/mocks/**` are WP0-owned and read-only for other work packages.
- Universal `load()` functions using `fetch` (e.g. via `graphqlClient.ts`) need `filterSerializedResponseHeaders` allow-listing `content-type` in `src/hooks.server.ts` — SvelteKit strips response headers from the tracked SSR fetch by default, which breaks urql's `fetchExchange` (it reads `content-type` to parse the response).

## Backend-target profiles

`graphqlClient.ts` always points at the same-origin `/api/graphql` proxy; the proxy itself resolves the real backend URL server-side via `GRAPHQL_URL` (`$env/dynamic/private`, falling back to `http://localhost:8080/graphql`), or serves mocks when `PUBLIC_USE_MOCKS=1`. Profiles are plain Vite mode env files:

- `.env.tower` → the deployed backend's `GRAPHQL_URL` for the Unraid box, loaded by `npm run dev:tower` (`vite dev --mode tower`).
- No `.env` file for the local profile — the code fallback already covers it, so plain `npm run dev` is "local".

To add another target: add `.env.<mode>` + a `"dev:<mode>": "vite dev --mode <mode>"` script.
