import { describe, expect, it } from 'vitest';
import type { TransactionSplit } from './graphql/types';
import { splitIndicator } from './splitView';

const part = (index: number): TransactionSplit => ({
	index,
	amount: '-10.00',
	category: {
		id: `c${index}`,
		slug: 'x',
		name: 'X',
		kind: 'EXPENSE'
	} as TransactionSplit['category']
});

describe('splitIndicator', () => {
	it('is null for a plain transaction', () => {
		expect(splitIndicator([])).toBeNull();
	});

	it('carries the part count', () => {
		expect(splitIndicator([part(0), part(1), part(2)])).toMatchObject({
			text: 'split · 3 parts',
			count: 3
		});
	});

	it('is singular for one part', () => {
		expect(splitIndicator([part(0)])?.text).toBe('split · 1 part');
	});
});
