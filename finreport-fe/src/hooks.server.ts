import type { Handle } from '@sveltejs/kit';
import { forwardGraphql } from '$lib/server/graphqlBackend';
import { ME_QUERY } from '$lib/graphql/queries';
import type { Me } from '$lib/graphql/types';

/**
 * Resolve `me` once per request (§6 "Auth handling") straight through the
 * shared `forwardGraphql` helper — not a self-HTTP round trip through
 * `/api/graphql` — and expose it as `event.locals.user` for the `(app)`
 * layout guard and any page that wants the current user.
 */
export const handle: Handle = async ({ event, resolve }) => {
	const result = await forwardGraphql(event, { query: ME_QUERY });
	const data = (result.body as { data?: { me: Me | null } } | undefined)?.data;
	event.locals.user = data?.me ?? null;
	return resolve(event, {
		// urql's fetchExchange reads the `content-type` response header to parse
		// GraphQL responses during SSR load()s; SvelteKit strips response headers
		// from the tracked fetch by default unless explicitly allow-listed here.
		filterSerializedResponseHeaders: (name) => name === 'content-type'
	});
};
