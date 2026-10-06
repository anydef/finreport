import { fail, redirect } from '@sveltejs/kit';
import { forwardGraphql, relaySetCookies } from '$lib/server/graphqlBackend';
import { LOGIN_MUTATION } from '$lib/graphql/queries';
import type { Actions, PageServerLoad } from './$types';

/** Only ever redirect to a same-site relative path — never an open redirect. */
function safeRedirectTarget(value: string | null): string {
	if (value && value.startsWith('/') && !value.startsWith('//')) return value;
	return '/';
}

export const load: PageServerLoad = ({ url, locals }) => {
	if (locals.user) {
		redirect(307, safeRedirectTarget(url.searchParams.get('redirectTo')));
	}
	return {};
};

export const actions: Actions = {
	default: async (event) => {
		const form = await event.request.formData();
		const username = String(form.get('username') ?? '').trim();
		const password = String(form.get('password') ?? '');

		if (!username || !password) {
			return fail(400, { error: 'Username and password are required.' });
		}

		const result = await forwardGraphql(event, {
			query: LOGIN_MUTATION,
			variables: { input: { username, password } }
		});
		const body = result.body as { errors?: { message: string }[] };
		if (body.errors?.length) {
			// Generic message regardless of cause (§4/§6): no "username not found".
			return fail(401, { error: 'Invalid username or password.' });
		}

		relaySetCookies(event, result.setCookies);
		redirect(303, safeRedirectTarget(event.url.searchParams.get('redirectTo')));
	}
};
