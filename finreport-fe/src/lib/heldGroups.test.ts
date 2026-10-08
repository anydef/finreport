import { describe, expect, it } from 'vitest';
import {
	expandedKey,
	groupFilter,
	isAssignable,
	needsCreating,
	parseView,
	proposalOf,
	proposedCategoryInput,
	reasonLabels,
	reviewHref
} from './heldGroups';

describe('view and URL', () => {
	it('defaults to groups and only recognises flat', () => {
		expect(parseView(null)).toBe('groups');
		expect(parseView('flat')).toBe('flat');
		expect(parseView('nonsense')).toBe('groups');
	});
	it('builds links; groups is the bare default and expand only applies to it', () => {
		expect(reviewHref('groups')).toBe('?');
		expect(reviewHref('flat')).toBe('?view=flat');
		expect(reviewHref('groups', 'a b')).toBe('?expand=a+b');
		expect(reviewHref('flat', 'x')).toBe('?view=flat');
	});
	it('maps the legacy counterpartyKey parameter onto expand', () => {
		expect(expandedKey(new URLSearchParams('expand=rewe'))).toBe('rewe');
		expect(expandedKey(new URLSearchParams('counterpartyKey=rewe'))).toBe('rewe');
		expect(expandedKey(new URLSearchParams(''))).toBeNull();
	});
});

describe('group actions', () => {
	it('filters to the queue plus the merchant key', () => {
		expect(groupFilter({ counterpartyKey: 'rewe' })).toEqual({
			needsReview: true,
			counterpartyKeys: ['rewe']
		});
	});
	it('refuses to build a filter for the keyless bucket', () => {
		expect(isAssignable({ counterpartyKey: null })).toBe(false);
		expect(() => groupFilter({ counterpartyKey: null })).toThrow();
	});
});

describe('proposalOf', () => {
	const base = { proposedCategoryPath: 'food.groceries', heldCount: 3 };
	it('is none without a path', () => {
		expect(proposalOf({ ...base, proposedCategoryPath: null, proposedCategoryVotes: 0 })).toEqual({
			kind: 'none'
		});
	});
	it('is unanimous only when every transaction agrees', () => {
		expect(proposalOf({ ...base, proposedCategoryVotes: 3 })).toEqual({
			kind: 'unanimous',
			path: 'food.groceries'
		});
	});
	it('is mixed when some disagree or have no proposal', () => {
		expect(proposalOf({ ...base, proposedCategoryVotes: 2 })).toEqual({
			kind: 'mixed',
			path: 'food.groceries',
			votes: 2,
			total: 3
		});
	});
});

describe('proposed categories', () => {
	it('derives name and parent from the dotted path', () => {
		expect(proposedCategoryInput('housing.co_working')).toEqual({
			slug: 'housing.co_working',
			name: 'Co Working',
			kind: 'EXPENSE',
			parentSlug: 'housing'
		});
		expect(proposedCategoryInput('fun').parentSlug).toBeUndefined();
	});
	it('creates only categories that do not exist', () => {
		expect(needsCreating('a.b', [{ slug: 'a.b' }])).toBe(false);
		expect(needsCreating('a.c', [{ slug: 'a.b' }])).toBe(true);
	});
	it('labels reasons', () => {
		expect(reasonLabels(['AMBIGUOUS', 'NEW_CATEGORY'])).toEqual(['Ambiguous', 'New category']);
	});
});
