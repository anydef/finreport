/**
 * Which `/admin/*` screens are for administrators, and what to say when the
 * server refuses.
 *
 * COSMETIC ONLY. Hiding a tab and redirecting a route stops an ordinary user
 * wandering into a screen that cannot work for them; it protects nothing.
 * The GraphQL endpoint is reachable directly, so the real control is the
 * server-side `AdminOnly` guard on the resolvers (`FORBIDDEN`). Never rely
 * on anything in this file for security.
 *
 * The split mirrors the backend's boundary: screens over shared state
 * (rules, the category taxonomy, user management) are admin-only; screens
 * over the caller's own data (nicknames, savings accounts, Amazon orders)
 * stay open to everyone.
 */

/** Admin-section paths whose backing resolvers are admin-only. */
export const ADMIN_ONLY_PATHS = ['/admin/rules', '/admin/categories', '/admin/users'] as const;

/** Where a non-admin lands inside the admin section. */
export const NON_ADMIN_LANDING_PATH = '/admin/aliases';

export const FORBIDDEN_MESSAGE =
	'This needs an administrator account. Ask an administrator to make the change.';

export function isAdminOnlyPath(pathname: string): boolean {
	return ADMIN_ONLY_PATHS.some((path) => pathname === path || pathname.startsWith(`${path}/`));
}

/** The admin sub-tabs the viewer may see. */
export function visibleAdminTabs<T extends { href: string }>(tabs: T[], isAdmin: boolean): T[] {
	return isAdmin ? tabs : tabs.filter((tab) => !isAdminOnlyPath(tab.href));
}

/** Where `/admin` itself should send the viewer. */
export function adminLandingPath(isAdmin: boolean): string {
	return isAdmin ? '/admin/rules' : NON_ADMIN_LANDING_PATH;
}

/** The redirect target for a non-admin on an admin-only path, else null. */
export function adminRedirectTarget(pathname: string, isAdmin: boolean): string | null {
	return !isAdmin && isAdminOnlyPath(pathname) ? NON_ADMIN_LANDING_PATH : null;
}

export interface ErrorLike {
	message?: string;
	graphQLErrors?: { message?: string; extensions?: Record<string, unknown> }[];
}

export function isForbidden(error: ErrorLike | null | undefined): boolean {
	return Boolean(error?.graphQLErrors?.some((e) => e.extensions?.code === 'FORBIDDEN'));
}

/** A user-facing message for a failed admin call: the honest reason when it
 * was a refusal, otherwise the caller's own fallback. */
export function adminErrorMessage(error: ErrorLike | null | undefined, fallback: string): string {
	return isForbidden(error) ? FORBIDDEN_MESSAGE : fallback;
}
