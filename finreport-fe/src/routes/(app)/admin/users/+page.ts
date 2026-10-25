import { env as publicEnv } from '$env/dynamic/public';
import type { PageLoad } from './$types';

/** No backend serves user management yet; only mock mode has data to show. */
export const load: PageLoad = () => ({ mockMode: publicEnv.PUBLIC_USE_MOCKS === '1' });
