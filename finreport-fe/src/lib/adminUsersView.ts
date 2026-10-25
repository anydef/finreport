/**
 * Pure logic for the mock-only `/admin/users` page. No Svelte, no I/O —
 * covered by `adminUsersView.test.ts`. The backend slice has to agree with
 * everything stated here.
 *
 * ## What the server enforces today (and so what this module may promise)
 *
 * Read from `finreport-rs/webapp/src/auth/user.rs` and `bin/user_admin.rs`:
 *
 * - **Username**: lowercased (`normalize_username`) and unique
 *   (`AuthError::UsernameTaken`). The CLI requires the `--username` flag to
 *   carry a value. Nothing else is checked: no length, charset or whitespace
 *   rule exists.
 * - **Password**: NOTHING is enforced. `hash_password` takes any
 *   `SecretString`, including an empty one (the CLI can read an empty line
 *   from stdin). There is no minimum length, no complexity rule. This module
 *   therefore states no password *policy*. The only checks are UX guards that
 *   are not policy claims: the field is not left empty (our own guard,
 *   stricter than the server) and the confirmation matches.
 *   Whether to introduce a real policy is a decision for the owner, to be made
 *   together with the backend slice.
 * - **Admin / delete**: the CLI has no promote, demote or delete command. The
 *   rules below for those (last admin) are this page's proposal for the
 *   backend slice, not current behaviour. Note `auth::bootstrap` re-promotes
 *   and re-links the configured admin on every restart.
 *
 * Nothing here handles a password value beyond checking it; it is never stored
 * or returned.
 */

import type { MockAccount, MockUser } from './graphql/usersMock';

export type UserAction = 'set-password' | 'link' | 'unlink' | 'promote' | 'demote' | 'delete';

export interface UserRow {
	id: string;
	username: string;
	displayName: string | null;
	isAdmin: boolean;
	disabled: boolean;
	linkedCount: number;
	/** "none", "all 4 accounts" or "2 of 4". */
	linkSummary: string;
	linkedLabels: string[];
	/** Date only (UTC), or "never". */
	lastLogin: string;
}

/** Display name for an account: its label, else `<source>:<external-id>`. */
export function accountName(account: MockAccount): string {
	return account.label ?? `${account.source}:${account.externalId}`;
}

/** Admins first, then by username (case-insensitive). Does not mutate. */
export function sortUsers(users: readonly MockUser[]): MockUser[] {
	return [...users].sort(
		(a, b) =>
			Number(b.isAdmin) - Number(a.isAdmin) ||
			a.username.toLowerCase().localeCompare(b.username.toLowerCase())
	);
}

function linkSummary(linked: number, total: number): string {
	if (linked === 0) return 'none';
	if (total > 0 && linked === total) return `all ${total} accounts`;
	return `${linked} of ${total}`;
}

/** Table rows. Linked ids that match no known account are ignored. */
export function toRows(users: readonly MockUser[], accounts: readonly MockAccount[]): UserRow[] {
	const byId = new Map(accounts.map((a) => [a.id, a]));
	return sortUsers(users).map((u) => {
		const linked = u.accountIds.map((id) => byId.get(id)).filter((a): a is MockAccount => !!a);
		return {
			id: u.id,
			username: u.username,
			displayName: u.displayName,
			isAdmin: u.isAdmin,
			disabled: u.disabled,
			linkedCount: linked.length,
			linkSummary: linkSummary(linked.length, accounts.length),
			linkedLabels: linked.map(accountName),
			lastLogin: u.lastLoginAt ? u.lastLoginAt.slice(0, 10) : 'never'
		};
	});
}

function plural(n: number, one: string, many: string): string {
	return `${n} ${n === 1 ? one : many}`;
}

/** "5 users, 1 admin, 1 without accounts". */
export function summarizeUsers(users: readonly MockUser[]): string {
	const admins = users.filter((u) => u.isAdmin).length;
	const unlinked = users.filter((u) => u.accountIds.length === 0).length;
	return `${plural(users.length, 'user', 'users')}, ${plural(admins, 'admin', 'admins')}, ${unlinked} without accounts`;
}

// --- validation -----------------------------------------------------------

/** The server's normalisation: lowercase only, no trimming. */
export function normalizeUsername(username: string): string {
	return username.toLowerCase();
}

/**
 * Errors for a new username. Only what the server really enforces: present,
 * and unique after lowercasing.
 */
export function validateUsername(username: string, existing: readonly string[]): string[] {
	if (username.length === 0) return ['Username is required.'];
	const wanted = normalizeUsername(username);
	if (existing.some((e) => normalizeUsername(e) === wanted)) {
		return [`The username "${wanted}" is already taken.`];
	}
	return [];
}

/**
 * Errors for a password pair. The server enforces no policy (see module doc);
 * these are UX guards only.
 */
export function validatePassword(password: string, confirmation: string): string[] {
	const errors: string[] = [];
	if (password.length === 0) errors.push('Password is required.');
	else if (password !== confirmation) errors.push('The two passwords do not match.');
	return errors;
}

export interface CreateUserErrors {
	username: string[];
	password: string[];
}

export function validateCreateUser(
	input: { username: string; password: string; confirmation: string },
	existingUsernames: readonly string[]
): { ok: boolean; errors: CreateUserErrors } {
	const errors = {
		username: validateUsername(input.username, existingUsernames),
		password: validatePassword(input.password, input.confirmation)
	};
	return { ok: errors.username.length === 0 && errors.password.length === 0, errors };
}

// --- action availability --------------------------------------------------

export interface Availability {
	available: boolean;
	/** Why not, when unavailable. */
	reason?: string;
}

const OK: Availability = { available: true };
const no = (reason: string): Availability => ({ available: false, reason });

/**
 * Whether `action` makes sense for `user` given everyone else. `link` and
 * `unlink` are about the user as a whole here (is there anything to link or
 * unlink); per-account choices are `canLinkAccount` / `canUnlinkAccount`.
 */
export function actionAvailability(
	action: UserAction,
	user: MockUser,
	users: readonly MockUser[],
	accounts: readonly MockAccount[]
): Availability {
	const otherAdmins = users.filter((u) => u.isAdmin && u.id !== user.id && !u.disabled).length;
	switch (action) {
		case 'set-password':
			return OK;
		case 'promote':
			return user.isAdmin ? no('Already an admin.') : OK;
		case 'demote':
			if (!user.isAdmin) return no('Not an admin.');
			return otherAdmins === 0 ? no('The last admin cannot be demoted.') : OK;
		case 'delete':
			if (user.isAdmin && otherAdmins === 0) return no('The last admin cannot be deleted.');
			return OK;
		case 'link':
			return user.accountIds.length >= accounts.length
				? no('Already linked to every account.')
				: OK;
		case 'unlink':
			return user.accountIds.length === 0 ? no('No linked accounts.') : OK;
	}
}

export function canLinkAccount(user: MockUser, accountId: string): Availability {
	return user.accountIds.includes(accountId) ? no('Already linked.') : OK;
}

export function canUnlinkAccount(user: MockUser, accountId: string): Availability {
	return user.accountIds.includes(accountId) ? OK : no('Not linked.');
}

/** True when unlinking `accountId` would leave the user with no accounts. */
export function unlinkLeavesNothing(user: MockUser, accountId: string): boolean {
	return user.accountIds.length === 1 && user.accountIds[0] === accountId;
}

// --- confirm wording ------------------------------------------------------

export interface ActionDescription {
	title: string;
	/** Sentences for the confirm step. */
	lines: string[];
	confirmLabel: string;
	destructive: boolean;
}

/** Words for the confirm step of an action. */
export function describeAction(
	action: UserAction,
	user: MockUser,
	accounts: readonly MockAccount[] = [],
	accountId?: string
): ActionDescription {
	const name = user.username;
	switch (action) {
		case 'delete': {
			const n = user.accountIds.length;
			return {
				title: `Delete ${name}?`,
				lines: [
					`${name} will no longer be able to sign in, and their sessions end.`,
					n > 0
						? `The ${plural(n, 'account link', 'account links')} are removed; the accounts and their transactions are not deleted.`
						: 'They have no account links.',
					'This cannot be undone.'
				],
				confirmLabel: 'Delete user',
				destructive: true
			};
		}
		case 'demote':
			return {
				title: `Remove admin rights from ${name}?`,
				lines: [
					`${name} keeps their login and account links but loses access to the admin pages.`,
					'Only another admin can restore it.'
				],
				confirmLabel: 'Demote',
				destructive: true
			};
		case 'promote':
			return {
				title: `Make ${name} an admin?`,
				lines: [`${name} will be able to manage users, rules and categories.`],
				confirmLabel: 'Promote',
				destructive: false
			};
		case 'set-password':
			return {
				title: `Set a new password for ${name}?`,
				lines: ['The old password stops working immediately.'],
				confirmLabel: 'Set password',
				destructive: false
			};
		case 'link':
			return {
				title: `Link accounts to ${name}?`,
				lines: [`${name} will see these accounts and their transactions.`],
				confirmLabel: 'Link',
				destructive: false
			};
		case 'unlink': {
			const account = accounts.find((a) => a.id === accountId);
			const what = account ? accountName(account) : 'this account';
			const lines = [`${name} will no longer see ${what} or its transactions.`];
			if (accountId && unlinkLeavesNothing(user, accountId)) {
				lines.push(`This is their only account, so ${name} would see nothing at all.`);
			}
			return {
				title: `Unlink ${what} from ${name}?`,
				lines,
				confirmLabel: 'Unlink',
				destructive: true
			};
		}
	}
}
