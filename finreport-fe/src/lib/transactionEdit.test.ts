import { describe, expect, it } from 'vitest';
import {
	applyCategoryResult,
	applyRecurringResult,
	applyTagsResult,
	categoryChangeWarning
} from './transactionEdit';
import { categoryOptionGroups } from './categoryTree';
import type { Category, Transaction } from './graphql/types';

const cat = (id: string, name: string, parentId: string | null, archived = false): Category =>
	({
		id,
		slug: name.toLowerCase(),
		name,
		kind: 'EXPENSE',
		parentId,
		depth: 0,
		archived
	}) as Category;

const base = {
	id: 't1',
	tags: ['a'],
	splits: [{ index: 0, amount: '-1', category: cat('c', 'X', null) }],
	label: {
		category: cat('c', 'X', null),
		source: 'LLM',
		rule: null,
		confidence: 0.4,
		status: 'NEEDS_REVIEW',
		reviewReason: 'LOW_CONFIDENCE',
		proposedCategoryPath: 'x',
		reasoning: 'because'
	},
	recurring: {
		isRecurring: false,
		source: 'AUTO',
		seriesId: null,
		cadence: null,
		medianAmount: null
	}
} as unknown as Transaction;

describe('applyCategoryResult', () => {
	const result = {
		label: { category: cat('f', 'Food', null), source: 'USER', status: 'RESOLVED' }
	} as never;

	it('replaces the label with a resolved user decision and drops stale review metadata', () => {
		const next = applyCategoryResult(base, result);
		expect(next.label?.source).toBe('USER');
		expect(next.label?.status).toBe('RESOLVED');
		expect(next.label?.category?.name).toBe('Food');
		expect(next.label?.confidence).toBeNull();
		expect(next.label?.reviewReason).toBeNull();
		expect(next.label?.reasoning).toBeNull();
	});

	it('clears splits and keeps other fields', () => {
		const next = applyCategoryResult(base, result);
		expect(next.splits).toEqual([]);
		expect(next.tags).toEqual(['a']);
	});
});

describe('applyTagsResult / applyRecurringResult', () => {
	it('replaces the tag set', () => {
		expect(applyTagsResult(base, { tags: ['b', 'c'] }).tags).toEqual(['b', 'c']);
	});
	it('replaces the recurring info', () => {
		const recurring = { ...base.recurring, isRecurring: true, source: 'USER' as const };
		expect(applyRecurringResult(base, { recurring }).recurring.isRecurring).toBe(true);
	});
});

describe('categoryChangeWarning', () => {
	it('is null without splits', () => {
		expect(categoryChangeWarning({ splits: [] })).toBeNull();
	});
	it('names the number of parts when splits exist', () => {
		expect(categoryChangeWarning(base)).toContain('1 parts');
	});
});

describe('categoryOptionGroups', () => {
	it('groups by top-level ancestor with path labels and skips archived', () => {
		const groups = categoryOptionGroups([
			cat('1', 'Food', null),
			cat('2', 'Groceries', '1'),
			cat('3', 'Old', '1', true),
			cat('4', 'Travel', null)
		]);
		expect(groups.map((g) => g.groupLabel)).toEqual(['Food', 'Travel']);
		expect(groups[0].options.map((o) => o.label)).toEqual(['Food', 'Food / Groceries']);
	});
});
