import { redirect } from '@sveltejs/kit';
import type { LayoutServerLoad } from './$types';

/**
 * Auth guard for every route under `(app)` (§6): `hooks.server.ts` already
 * resolved `me` into `event.locals.user`; redirect to `/login` with a
 * `redirectTo` so the user lands back where they came from after signing in.
 */
export const load: LayoutServerLoad = ({ locals, url }) => {
	if (!locals.user) {
		const redirectTo = `${url.pathname}${url.search}`;
		redirect(307, `/login?redirectTo=${encodeURIComponent(redirectTo)}`);
	}
	return { user: locals.user };
};
