import { describe, expect, it } from 'vitest';
import {
	expandedIban,
	expandedKey,
	groupId,
	isExpanded,
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
	it('leaves the IBAN off a key that is not split, even when the group has one', () => {
		expect(
			groupFilter({ counterpartyKey: 'rewe', counterpartyIban: 'DE00 1', ibanDisambiguated: false })
		).toEqual({ needsReview: true, counterpartyKeys: ['rewe'] });
	});
	it('adds the IBAN for a split key so a sibling group is not selected', () => {
		expect(
			groupFilter({ counterpartyKey: 'loan', counterpartyIban: 'DE00 1', ibanDisambiguated: true })
		).toEqual({ needsReview: true, counterpartyKeys: ['loan'], counterpartyIbans: ['DE00 1'] });
	});
	it('uses hasCounterpartyIban=false, never a sentinel IBAN, for the split key without one', () => {
		expect(
			groupFilter({ counterpartyKey: 'loan', counterpartyIban: null, ibanDisambiguated: true })
		).toEqual({ needsReview: true, counterpartyKeys: ['loan'], hasCounterpartyIban: false });
	});
	it('refuses to build a filter for a keyless group, with or without an IBAN', () => {
		expect(isAssignable({ counterpartyKey: null })).toBe(false);
		expect(() => groupFilter({ counterpartyKey: null })).toThrow();
		expect(() =>
			groupFilter({ counterpartyKey: null, counterpartyIban: 'DE00 1', ibanDisambiguated: true })
		).toThrow();
	});
	it('stays unassignable for a keyless group carrying an IBAN: an IBAN-only filter would also hit keyed rows', () => {
		expect(isAssignable({ counterpartyKey: null, counterpartyIban: 'DE00 1' } as never)).toBe(
			false
		);
		expect(isAssignable({ counterpartyKey: 'loan' })).toBe(true);
	});
});

describe('split groups in the URL', () => {
	it('names one group of a split key with expandIban, and the no-IBAN one with an empty value', () => {
		expect(reviewHref('groups', 'loan', 'DE00 1')).toBe('?expand=loan&expandIban=DE00+1');
		expect(reviewHref('groups', 'loan', null)).toBe('?expand=loan&expandIban=');
		expect(reviewHref('groups', 'loan')).toBe('?expand=loan');
		expect(reviewHref('groups', null, 'DE00 1')).toBe('?');
	});
	it('reads expandIban back as absent / none / a value', () => {
		expect(expandedIban(new URLSearchParams('expand=loan'))).toBeUndefined();
		expect(expandedIban(new URLSearchParams('expand=loan&expandIban='))).toBeNull();
		expect(expandedIban(new URLSearchParams('expand=loan&expandIban=DE00+1'))).toBe('DE00 1');
	});
	it('opens only the named group of a split key', () => {
		const a = { counterpartyKey: 'loan', counterpartyIban: 'DE00 1' };
		const none = { counterpartyKey: 'loan', counterpartyIban: null };
		expect(isExpanded(a, 'loan', 'DE00 1')).toBe(true);
		expect(isExpanded(none, 'loan', 'DE00 1')).toBe(false);
		expect(isExpanded(none, 'loan', null)).toBe(true);
		expect(isExpanded(a, 'loan', undefined)).toBe(true);
		expect(isExpanded(a, 'other', undefined)).toBe(false);
		expect(isExpanded({ counterpartyKey: null }, null, undefined)).toBe(false);
	});
	it('gives sibling groups distinct list keys', () => {
		const ids = [
			groupId({ counterpartyKey: 'loan', counterpartyIban: 'DE00 1' }),
			groupId({ counterpartyKey: 'loan', counterpartyIban: null }),
			groupId({ counterpartyKey: null, counterpartyIban: null }),
			groupId({ counterpartyKey: null, counterpartyIban: 'DE00 1' })
		];
		expect(new Set(ids).size).toBe(4);
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
