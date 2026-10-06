# finreport-fe

SvelteKit frontend for finreport, consuming the GraphQL backend in `../finreport-rs/webapp`.

## Stack

- SvelteKit 2 + Svelte 5, TypeScript
- `@urql` GraphQL client (`src/lib/graphqlClient.ts`); backend URL via `PUBLIC_GRAPHQL_URL`
- Tailwind CSS 4
- Storybook 9 for component development (`src/stories/`)
- Playwright for e2e tests (`e2e/`)

## Commands

```bash
npm run dev            # local dev server, local profile (talks to backend on localhost:8080)
npm run dev:tower       # local dev server, tower profile (talks to the deployed Unraid backend)
npm run build / preview
npm run check           # svelte-kit sync + svelte-check (type checking)
npm run lint             # prettier --check + eslint
npm run format            # prettier --write
npm run test              # test:unit then test:e2e
npm run test:unit          # vitest — pure logic only, see Conventions
npm run test:e2e           # playwright
npm run storybook         # component dev server
```

## Conventions

- Run `npm run check` and `npm run lint` before considering frontend work done.
- Vitest (`npm run test:unit`) covers pure, non-Svelte logic extracted into `src/lib/*.ts` (e.g. `src/lib/transactions.ts`). There's no component-rendering test setup (no `@testing-library/svelte`/jsdom) — component-level coverage still comes from Storybook/Playwright. Configured via a `test` block in `vite.config.ts` (not a separate `vitest.config.ts`) so it shares the real `sveltekit()`/`tailwindcss()` plugin setup — including `$lib`/`$app`/`$env` alias resolution — instead of a hand-rolled duplicate that would drift from it.
- GraphQL operations go through `graphqlClient.ts`; keep the client in sync with schema changes made in `webapp/src/graphql/`.
- Universal `load()` functions using `fetch` (e.g. via `graphqlClient.ts`) need `filterSerializedResponseHeaders` allow-listing `content-type` in `src/hooks.server.ts` — SvelteKit strips response headers from the tracked SSR fetch by default, which breaks urql's `fetchExchange` (it reads `content-type` to parse the response).

## Backend-target profiles

`graphqlClient.ts` reads `PUBLIC_GRAPHQL_URL` via `$env/dynamic/public`, falling back to `http://localhost:8080/graphql`. Profiles are plain Vite mode env files:

- `.env.tower` → `PUBLIC_GRAPHQL_URL` for the deployed backend on the Unraid box, loaded by `npm run dev:tower` (`vite dev --mode tower`).
- No `.env` file for the local profile — the code fallback already covers it, so plain `npm run dev` is "local".

To add another target: add `.env.<mode>` + a `"dev:<mode>": "vite dev --mode <mode>"` script.
