import { redirect } from '@sveltejs/kit';
import type { PageLoad } from './$types';

/** Review was promoted to a top-level tab; keep old links and bookmarks working. */
export const load: PageLoad = () => {
	redirect(308, '/review');
};
