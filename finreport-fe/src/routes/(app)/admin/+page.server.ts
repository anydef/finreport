import { redirect } from '@sveltejs/kit';
import type { PageServerLoad } from './$types';

/** `/admin` has no content of its own — Review is the default admin screen. */
export const load: PageServerLoad = async () => {
	redirect(307, '/admin/review');
};
