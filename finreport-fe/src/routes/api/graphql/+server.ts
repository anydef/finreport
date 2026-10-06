/**
 * Same-origin GraphQL proxy (§4 CSRF, §6). The browser talks to the backend
 * through this route instead of cross-origin, so the session cookie can stay
 * `SameSite=Lax` instead of needing `SameSite=None; Secure` (which plain-HTTP
 * localhost can't do). SvelteKit's own `csrf.checkOrigin` (kept on in
 * `svelte.config.js`) already rejects a forged cross-site *form* submission
 * before this handler runs; the `Content-Type` check below additionally
 * blocks a cross-site request from ever presenting as `application/json` in
 * the first place (that content type isn't a CORS-simple request, so a
 * foreign page cannot send it without a preflight the browser will block).
 *
 * `PUBLIC_USE_MOCKS=1` short-circuits to `forwardGraphql`'s mock responses —
 * no network call to a backend at all (§9 WP5).
 */
import { error, json, type RequestHandler } from '@sveltejs/kit';
import { forwardGraphql, type GraphqlRequestBody } from '$lib/server/graphqlBackend';

export const POST: RequestHandler = async (event) => {
	const contentType = event.request.headers.get('content-type') ?? '';
	if (!contentType.toLowerCase().startsWith('application/json')) {
		error(415, 'Content-Type must be application/json');
	}

	let body: GraphqlRequestBody;
	try {
		body = await event.request.json();
	} catch {
		error(400, 'Invalid JSON body');
	}
	if (typeof body?.query !== 'string') {
		error(400, 'Missing GraphQL query');
	}

	const result = await forwardGraphql(event, body);

	const response = json(result.body, { status: result.status });
	for (const cookie of result.setCookies) {
		response.headers.append('set-cookie', cookie);
	}
	return response;
};
