import { redirect } from '@sveltejs/kit';
import { adminRedirectTarget } from '$lib/adminAccess';
import type { LayoutServerLoad } from './$types';

/**
 * Cosmetic guard (see `$lib/adminAccess`): sends a non-admin away from the
 * screens that cannot work for them. The real control is the server-side
 * `AdminOnly` gate on the GraphQL resolvers - this endpoint is reachable
 * without ever loading a page.
 */
export const load: LayoutServerLoad = ({ locals, url }) => {
	const target = adminRedirectTarget(url.pathname, Boolean(locals.user?.isAdmin));
	if (target) redirect(307, target);
	return { isAdmin: Boolean(locals.user?.isAdmin) };
};
