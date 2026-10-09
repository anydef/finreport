import { describe, expect, it } from 'vitest';
import {
	MAX_ALIAS_LENGTH,
	accountOptionLabel,
	aliasError,
	describeAliasTarget,
	kindLabel,
	sortAliases
} from './aliasView';
import type { DisplayAlias } from './graphql/types';

const alias = (over: Partial<DisplayAlias>): DisplayAlias => ({
	kind: 'COUNTERPARTY',
	key: 'amazon',
	alias: 'Shopping',
	rawName: 'Amazon EU',
	transactionCount: 3,
	updatedAt: '2024-06-01T00:00:00Z',
	...over
});

describe('aliasError', () => {
	it('accepts a plain nickname', () => {
		expect(aliasError('  Mum ')).toBeNull();
	});
	it('rejects empty and over-long input', () => {
		expect(aliasError('   ')).not.toBeNull();
		expect(aliasError('x'.repeat(MAX_ALIAS_LENGTH))).toBeNull();
		expect(aliasError('x'.repeat(MAX_ALIAS_LENGTH + 1))).not.toBeNull();
	});
	it('rejects control characters', () => {
		expect(aliasError('a\nb')).not.toBeNull();
	});
});

describe('describeAliasTarget', () => {
	it('shows the bank name and a pluralised count for merchants', () => {
		expect(describeAliasTarget(alias({}))).toBe('Bank name: Amazon EU · 3 transactions');
		expect(describeAliasTarget(alias({ transactionCount: 1 }))).toBe(
			'Bank name: Amazon EU · 1 transaction'
		);
	});
	it('omits the count for accounts', () => {
		expect(
			describeAliasTarget(alias({ kind: 'ACCOUNT', rawName: 'Main', transactionCount: null }))
		).toBe('Bank name: Main');
	});
});

describe('sortAliases and labels', () => {
	it('sorts case-insensitively without mutating the input', () => {
		const input = [alias({ alias: 'zeta' }), alias({ alias: 'Alpha' })];
		expect(sortAliases(input).map((a) => a.alias)).toEqual(['Alpha', 'zeta']);
		expect(input[0].alias).toBe('zeta');
	});
	it('names the kinds for people', () => {
		expect(kindLabel('ACCOUNT')).toBe('My account');
		expect(kindLabel('COUNTERPARTY')).toBe('Merchant');
	});
});

describe('accountOptionLabel', () => {
	it('prefers label plus IBAN and falls back down the chain', () => {
		const base = { label: null, iban: null, displayId: null, id: 'uuid' };
		expect(accountOptionLabel({ ...base, label: 'Main', iban: 'DE1' })).toBe('Main (DE1)');
		expect(accountOptionLabel({ ...base, iban: 'DE1' })).toBe('DE1');
		expect(accountOptionLabel({ ...base, displayId: '42' })).toBe('42');
		expect(accountOptionLabel(base)).toBe('uuid');
	});
});
