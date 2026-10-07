import { redirect } from '@sveltejs/kit';
import type { PageServerLoad } from './$types';

/** `/admin` has no content of its own — Rules is the default admin screen (Review is a top-level tab). */
export const load: PageServerLoad = async () => {
	redirect(307, '/admin/rules');
};
