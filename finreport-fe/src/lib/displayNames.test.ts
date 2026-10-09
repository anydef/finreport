import { describe, expect, it } from 'vitest';
import {
	accountLabel,
	counterpartyLabel,
	rawCounterpartyName,
	relabelAccountNodes
} from './displayNames';

describe('counterpartyLabel', () => {
	it('prefers the nickname', () => {
		expect(
			counterpartyLabel({
				counterpartyName: 'BACKEREI MUELLER GMBH',
				counterpartyDisplayName: 'Bakery'
			})
		).toBe('Bakery');
	});
	it('falls back to the bank name, then null', () => {
		expect(counterpartyLabel({ counterpartyName: 'Lidl' })).toBe('Lidl');
		expect(counterpartyLabel({ counterpartyName: null, counterpartyDisplayName: null })).toBeNull();
	});
});

describe('rawCounterpartyName', () => {
	it('is the bank name only when a nickname differs from it', () => {
		expect(rawCounterpartyName({ counterpartyName: 'X GMBH', counterpartyDisplayName: 'X' })).toBe(
			'X GMBH'
		);
		expect(
			rawCounterpartyName({ counterpartyName: 'Lidl', counterpartyDisplayName: 'Lidl' })
		).toBeNull();
		expect(rawCounterpartyName({ counterpartyName: 'Lidl' })).toBeNull();
		expect(
			rawCounterpartyName({ counterpartyName: null, counterpartyDisplayName: 'X' })
		).toBeNull();
	});
});

describe('accountLabel', () => {
	it('prefers displayName and keeps the old fallbacks', () => {
		const base = { id: 'a', label: 'Main', displayId: '123' };
		expect(accountLabel({ ...base, displayName: 'Everyday' })).toBe('Everyday');
		expect(accountLabel(base)).toBe('Main');
		expect(accountLabel({ id: 'a', label: null, displayId: null })).toBe('a');
	});
});

describe('relabelAccountNodes', () => {
	const nodes = [
		{ label: 'Main', refType: 'account', refId: 'a1' },
		{ label: 'Joint', refType: 'account', refId: 'a2' },
		{ label: 'Rewe', refType: 'counterparty', refId: 'a1' },
		{ label: 'Other', refType: null, refId: null }
	];
	it('renames only account nodes with a known account', () => {
		const out = relabelAccountNodes(nodes, [{ id: 'a1', displayName: 'Everyday' }]);
		expect(out.map((n) => n.label)).toEqual(['Everyday', 'Joint', 'Rewe', 'Other']);
	});
});
