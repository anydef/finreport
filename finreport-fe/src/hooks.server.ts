import type { Handle } from '@sveltejs/kit';

// urql's fetchExchange reads the `content-type` response header to parse GraphQL
// responses during SSR load()s; SvelteKit strips response headers from the
// tracked fetch by default unless explicitly allow-listed here.
export const handle: Handle = ({ event, resolve }) =>
	resolve(event, { filterSerializedResponseHeaders: (name) => name === 'content-type' });
