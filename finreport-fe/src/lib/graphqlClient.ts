import { createClient, cacheExchange, fetchExchange } from '@urql/core';

/**
 * Always talks to the same-origin `/api/graphql` proxy (§6) — never directly
 * to the backend — so the session cookie can stay `SameSite=Lax`.
 * `credentials: 'include'` is what makes the browser attach that cookie on
 * same-origin requests when the page itself was loaded over `fetch` (SSR
 * `load()`s get it automatically via the request's own cookie header).
 */
export const createGraphqlClient = (fetch: typeof globalThis.fetch) =>
	createClient({
		url: '/api/graphql',
		exchanges: [cacheExchange, fetchExchange],
		fetch,
		fetchOptions: { credentials: 'include' }
	});
