import { redirect } from '@sveltejs/kit';
import { adminLandingPath } from '$lib/adminAccess';
import type { PageServerLoad } from './$types';

/** `/admin` has no content of its own — Rules is the default admin screen (Nicknames for a non-admin, who cannot use Rules) (Review is a top-level tab). */
export const load: PageServerLoad = async ({ locals }) => {
	redirect(307, adminLandingPath(Boolean(locals.user?.isAdmin)));
};
