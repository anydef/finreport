import { describe, expect, it } from 'vitest';
import { MOCK_ACCOUNTS, MOCK_USERS, type MockUser } from './graphql/usersMock';
import {
	actionAvailability,
	canLinkAccount,
	canUnlinkAccount,
	describeAction,
	normalizeUsername,
	sortUsers,
	summarizeUsers,
	toRows,
	unlinkLeavesNothing,
	validateCreateUser,
	validatePassword,
	validateUsername
} from './adminUsersView';

const user = (over: Partial<MockUser>): MockUser => ({
	id: 'u',
	username: 'u',
	displayName: null,
	isAdmin: false,
	disabled: false,
	lastLoginAt: null,
	accountIds: [],
	...over
});
const byName = (n: string) => MOCK_USERS.find((u) => u.username === n)!;

describe('sortUsers / toRows', () => {
	it('puts admins first, then alphabetical, without mutating', () => {
		const copy = [...MOCK_USERS];
		const names = sortUsers(MOCK_USERS).map((u) => u.username);
		expect(names[0]).toBe('admin');
		expect(names.slice(1)).toEqual([...names.slice(1)].sort());
		expect(MOCK_USERS).toEqual(copy);
	});

	it('summarises links as none / all / n of m', () => {
		const rows = toRows(MOCK_USERS, MOCK_ACCOUNTS);
		const get = (n: string) => rows.find((r) => r.username === n)!;
		expect(get('admin').linkSummary).toBe('all 4 accounts');
		expect(get('alex.example').linkSummary).toBe('2 of 4');
		expect(get('dev').linkSummary).toBe('none');
		expect(get('dev').lastLogin).toBe('never');
		expect(get('alex.example').lastLogin).toBe('2026-10-22');
	});

	it('names an unlabelled account by source:external-id and ignores unknown ids', () => {
		const rows = toRows([user({ accountIds: ['acc-4', 'ghost'] })], MOCK_ACCOUNTS);
		expect(rows[0].linkedCount).toBe(1);
		expect(rows[0].linkedLabels).toEqual(['example-bank:DE00 0000 0000 0000 0004']);
	});
});

describe('summarizeUsers', () => {
	it('counts users, admins and unlinked', () => {
		expect(summarizeUsers(MOCK_USERS)).toBe('5 users, 1 admin, 1 without accounts');
		expect(summarizeUsers([])).toBe('0 users, 0 admins, 0 without accounts');
	});
});

describe('validateUsername', () => {
	it('requires a value', () => {
		expect(validateUsername('', [])).toEqual(['Username is required.']);
	});
	it('does not trim or restrict charset, matching the server', () => {
		expect(validateUsername('  odd name!  ', [])).toEqual([]);
	});
	it('rejects a duplicate case-insensitively, reporting the normalised name', () => {
		expect(validateUsername('ADMIN', ['admin'])).toEqual([
			'The username "admin" is already taken.'
		]);
		expect(validateUsername('Dev', ['DEV'])).toHaveLength(1);
	});
	it('lowercases like the server', () => {
		expect(normalizeUsername('Alex.Example')).toBe('alex.example');
	});
});

describe('validatePassword', () => {
	it('imposes no length or complexity rule', () => {
		expect(validatePassword('a', 'a')).toEqual([]);
	});
	it('requires a value and a matching confirmation', () => {
		expect(validatePassword('', '')).toEqual(['Password is required.']);
		expect(validatePassword('x', 'y')).toEqual(['The two passwords do not match.']);
	});
});

describe('validateCreateUser', () => {
	it('is ok only when both parts are', () => {
		const ok = validateCreateUser({ username: 'new', password: 'p', confirmation: 'p' }, ['admin']);
		expect(ok.ok).toBe(true);
		const bad = validateCreateUser({ username: 'admin', password: '', confirmation: '' }, [
			'admin'
		]);
		expect(bad.ok).toBe(false);
		expect(bad.errors.username).toHaveLength(1);
		expect(bad.errors.password).toHaveLength(1);
	});
});

describe('actionAvailability', () => {
	const avail = (a: Parameters<typeof actionAvailability>[0], u: MockUser) =>
		actionAvailability(a, u, MOCK_USERS, MOCK_ACCOUNTS);

	it('blocks demoting and deleting the last admin', () => {
		expect(avail('demote', byName('admin')).available).toBe(false);
		expect(avail('delete', byName('admin')).reason).toMatch(/last admin/);
	});
	it('allows them once another active admin exists', () => {
		const users = [...MOCK_USERS, user({ id: 'x', username: 'x', isAdmin: true })];
		expect(actionAvailability('demote', byName('admin'), users, MOCK_ACCOUNTS).available).toBe(
			true
		);
		expect(actionAvailability('delete', byName('admin'), users, MOCK_ACCOUNTS).available).toBe(
			true
		);
	});
	it('does not count a disabled admin as a remaining admin', () => {
		const users = [...MOCK_USERS, user({ id: 'x', username: 'x', isAdmin: true, disabled: true })];
		expect(actionAvailability('demote', byName('admin'), users, MOCK_ACCOUNTS).available).toBe(
			false
		);
	});
	it('gates promote/demote on current role', () => {
		expect(avail('promote', byName('admin')).available).toBe(false);
		expect(avail('promote', byName('dev')).available).toBe(true);
		expect(avail('demote', byName('dev')).available).toBe(false);
	});
	it('gates link/unlink on what is there to change', () => {
		expect(avail('link', byName('admin')).available).toBe(false);
		expect(avail('unlink', byName('dev')).available).toBe(false);
		expect(avail('link', byName('dev')).available).toBe(true);
		expect(avail('unlink', byName('alex.example')).available).toBe(true);
	});
	it('always allows set-password', () => {
		expect(avail('set-password', byName('dev')).available).toBe(true);
	});
	it('checks single accounts', () => {
		const alex = byName('alex.example');
		expect(canLinkAccount(alex, 'acc-1').available).toBe(false);
		expect(canLinkAccount(alex, 'acc-2').available).toBe(true);
		expect(canUnlinkAccount(alex, 'acc-2').available).toBe(false);
		expect(canUnlinkAccount(alex, 'acc-1').available).toBe(true);
	});
});

describe('describeAction', () => {
	it('marks delete and demote destructive and says what survives', () => {
		const d = describeAction('delete', byName('alex.example'), MOCK_ACCOUNTS);
		expect(d.destructive).toBe(true);
		expect(d.title).toBe('Delete alex.example?');
		expect(d.lines.join(' ')).toMatch(/2 account links.*not deleted/);
		expect(describeAction('delete', byName('dev')).lines.join(' ')).toMatch(/no account links/);
		expect(describeAction('demote', byName('admin')).destructive).toBe(true);
	});
	it('warns when an unlink would leave the user with nothing', () => {
		const robin = byName('robin.sample');
		expect(unlinkLeavesNothing(robin, 'acc-2')).toBe(true);
		const d = describeAction('unlink', robin, MOCK_ACCOUNTS, 'acc-2');
		expect(d.lines.join(' ')).toMatch(/see nothing at all/);
		const alex = describeAction('unlink', byName('alex.example'), MOCK_ACCOUNTS, 'acc-1');
		expect(alex.lines.join(' ')).not.toMatch(/nothing at all/);
	});
	it('treats promote, set-password and link as non-destructive', () => {
		for (const a of ['promote', 'set-password', 'link'] as const) {
			expect(describeAction(a, byName('dev')).destructive).toBe(false);
		}
	});
});
