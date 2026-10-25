import { describe, expect, it } from 'vitest';
import {
	FORBIDDEN_MESSAGE,
	adminErrorMessage,
	adminLandingPath,
	adminRedirectTarget,
	isAdminOnlyPath,
	isForbidden,
	visibleAdminTabs
} from './adminAccess';

const tabs = [
	{ label: 'Rules', href: '/admin/rules' },
	{ label: 'Categories', href: '/admin/categories' },
	{ label: 'Nicknames', href: '/admin/aliases' },
	{ label: 'Accounts', href: '/admin/accounts' },
	{ label: 'Users', href: '/admin/users' },
	{ label: 'Amazon', href: '/admin/amazon' }
];

const forbidden = { graphQLErrors: [{ extensions: { code: 'FORBIDDEN' } }] };

describe('admin access (cosmetic; the server is the control)', () => {
	it('classifies only the shared-state screens as admin-only', () => {
		expect(isAdminOnlyPath('/admin/rules')).toBe(true);
		expect(isAdminOnlyPath('/admin/categories/')).toBe(true);
		expect(isAdminOnlyPath('/admin/users')).toBe(true);
		for (const open of ['/admin/aliases', '/admin/accounts', '/admin/amazon', '/review', '/']) {
			expect(isAdminOnlyPath(open)).toBe(false);
		}
		expect(isAdminOnlyPath('/admin/rulesets')).toBe(false);
	});

	it('shows every tab to an admin and only the per-user ones to everyone else', () => {
		expect(visibleAdminTabs(tabs, true)).toEqual(tabs);
		expect(visibleAdminTabs(tabs, false).map((t) => t.label)).toEqual([
			'Nicknames',
			'Accounts',
			'Amazon'
		]);
	});

	it('redirects a non-admin off an admin-only path and leaves everyone else alone', () => {
		expect(adminRedirectTarget('/admin/rules', false)).toBe('/admin/aliases');
		expect(adminRedirectTarget('/admin/rules', true)).toBeNull();
		expect(adminRedirectTarget('/admin/aliases', false)).toBeNull();
	});

	it('lands each kind of viewer on a screen they can use', () => {
		expect(adminLandingPath(true)).toBe('/admin/rules');
		expect(adminLandingPath(false)).toBe('/admin/aliases');
	});

	it('recognises the FORBIDDEN code and nothing else', () => {
		expect(isForbidden(forbidden)).toBe(true);
		expect(isForbidden({ graphQLErrors: [{ extensions: { code: 'VALIDATION' } }] })).toBe(false);
		expect(isForbidden({ message: 'boom' })).toBe(false);
		expect(isForbidden(null)).toBe(false);
		expect(isForbidden(undefined)).toBe(false);
	});

	it('explains a refusal honestly and keeps the fallback for other failures', () => {
		expect(adminErrorMessage(forbidden, 'Failed to load rules.')).toBe(FORBIDDEN_MESSAGE);
		expect(adminErrorMessage({ message: 'x' }, 'Failed to load rules.')).toBe(
			'Failed to load rules.'
		);
	});
});
