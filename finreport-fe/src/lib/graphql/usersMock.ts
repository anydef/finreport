/**
 * Synthetic sample data for the mock-only `/admin/users` page. Nothing here is
 * real: invented usernames, placeholder identifiers, and no passwords of any
 * kind. The SDL has no user operations yet, so this module is the page's only
 * data source (see `adminUsersView.ts`).
 *
 * Shapes follow the `user-admin` CLI's view of the world: an `app_user` has a
 * username, optional display name, `is_admin` and `disabled` flags, and is
 * linked to accounts by `<source>:<external-id>`.
 */

export interface MockUser {
	id: string;
	username: string;
	displayName: string | null;
	isAdmin: boolean;
	disabled: boolean;
	/** ISO timestamp, or null when the user has never signed in. */
	lastLoginAt: string | null;
	/** Ids of the {@link MockAccount}s this user can see. */
	accountIds: string[];
}

export interface MockAccount {
	id: string;
	/** Matches the CLI's `<source>:<external-id>` selector. */
	source: string;
	externalId: string;
	label: string | null;
}

export const MOCK_ACCOUNTS: MockAccount[] = [
	{
		id: 'acc-1',
		source: 'comdirect',
		externalId: 'DE00 0000 0000 0000 0001',
		label: 'Sample current'
	},
	{
		id: 'acc-2',
		source: 'comdirect',
		externalId: 'DE00 0000 0000 0000 0002',
		label: 'Sample savings'
	},
	{
		id: 'acc-3',
		source: 'comdirect',
		externalId: 'DE00 0000 0000 0000 0003',
		label: 'Sample joint'
	},
	{ id: 'acc-4', source: 'example-bank', externalId: 'DE00 0000 0000 0000 0004', label: null }
];

export const MOCK_USERS: MockUser[] = [
	{
		id: 'usr-1',
		username: 'admin',
		displayName: 'Sample Admin',
		isAdmin: true,
		disabled: false,
		lastLoginAt: '2026-10-24T08:12:00Z',
		accountIds: ['acc-1', 'acc-2', 'acc-3', 'acc-4']
	},
	{
		id: 'usr-2',
		username: 'alex.example',
		displayName: 'Alex Example',
		isAdmin: false,
		disabled: false,
		lastLoginAt: '2026-10-22T17:40:00Z',
		accountIds: ['acc-1', 'acc-3']
	},
	{
		id: 'usr-3',
		username: 'robin.sample',
		displayName: 'Robin Sample',
		isAdmin: false,
		disabled: false,
		lastLoginAt: '2026-09-30T06:05:00Z',
		accountIds: ['acc-2']
	},
	{
		id: 'usr-4',
		username: 'dev',
		displayName: null,
		isAdmin: false,
		disabled: false,
		lastLoginAt: null,
		accountIds: []
	},
	{
		id: 'usr-5',
		username: 'kim.placeholder',
		displayName: 'Kim Placeholder',
		isAdmin: false,
		disabled: true,
		lastLoginAt: '2026-03-02T12:00:00Z',
		accountIds: ['acc-1']
	}
];
