import { describe, expect, it } from 'vitest';
import {
	buildCategoryTree,
	canHaveChildren,
	flattenCategoryTreeWithPath,
	groupRulesByState,
	describeReach,
	isAutoApproved,
	mergeConditions,
	MAX_CATEGORY_DEPTH,
	sortByRecentlyCreated,
	sortRulesBySpecificity,
	specificityScore,
	summarizeConditions
} from './rulesView';
import type { Category, Rule } from './graphql/types';

function rule(overrides: Partial<Rule> & { id: string }): Rule {
	return {
		name: 'Test rule',
		category: {
			id: 'cat-1',
			slug: 'food',
			name: 'Food',
			kind: 'EXPENSE',
			parentId: null,
			depth: 1,
			archived: false,
			origin: 'seed'
		},
		conditions: {},
		priority: 0,
		state: 'ACTIVE',
		origin: 'USER',
		autoApproved: false,
		confidence: null,
		evidenceCount: 0,
		matchingTransactionCount: 0,
		createdAt: '2024-01-01T00:00:00Z',
		...overrides
	};
}

describe('specificityScore', () => {
	it('weighs counterparty_key and counterparty_iban at 2', () => {
		expect(specificityScore({ counterparty_key: 'lidl' })).toBe(2);
		expect(specificityScore({ counterparty_iban: 'DE12' })).toBe(2);
	});

	it('weighs a bounded amount range at 1, only when both bounds are present', () => {
		expect(specificityScore({ amount_min: '0' })).toBe(0);
		expect(specificityScore({ amount_min: '0', amount_max: '50' })).toBe(1);
	});

	it('sums every present condition', () => {
		expect(
			specificityScore({
				counterparty_key: 'lidl',
				amount_min: '0',
				amount_max: '50',
				direction: 'SPENDING'
			})
		).toBe(4);
	});

	it('returns 0 for no conditions', () => {
		expect(specificityScore({})).toBe(0);
	});
});

describe('sortRulesBySpecificity', () => {
	it('orders by priority descending first', () => {
		const low = rule({ id: 'b', priority: 0 });
		const high = rule({ id: 'a', priority: 5 });
		expect(sortRulesBySpecificity([low, high])).toEqual([high, low]);
	});

	it('breaks a priority tie by specificity descending', () => {
		const vague = rule({ id: 'b', conditions: { direction: 'SPENDING' } });
		const specific = rule({ id: 'a', conditions: { counterparty_key: 'lidl' } });
		expect(sortRulesBySpecificity([vague, specific])).toEqual([specific, vague]);
	});

	it('breaks a full tie by id ascending, never row order', () => {
		const b = rule({ id: 'b' });
		const a = rule({ id: 'a' });
		expect(sortRulesBySpecificity([b, a])).toEqual([a, b]);
	});
});

describe('summarizeConditions', () => {
	it('renders one line per present condition', () => {
		expect(
			summarizeConditions({ counterparty_key: 'lidl', direction: 'SPENDING', amount_max: '50' })
		).toEqual(['counterparty = "lidl"', 'direction = SPENDING', 'amount … – 50']);
	});

	it('returns an empty list for no conditions', () => {
		expect(summarizeConditions({})).toEqual([]);
	});
});

describe('isAutoApproved', () => {
	it('is true only for learned + autoApproved rules', () => {
		expect(isAutoApproved(rule({ id: '1', origin: 'LEARNED', autoApproved: true }))).toBe(true);
		expect(isAutoApproved(rule({ id: '2', origin: 'LEARNED', autoApproved: false }))).toBe(false);
		expect(isAutoApproved(rule({ id: '3', origin: 'USER', autoApproved: true }))).toBe(false);
	});
});

describe('groupRulesByState', () => {
	it('buckets rules by state and sorts each bucket', () => {
		const active1 = rule({ id: 'b', state: 'ACTIVE', priority: 0 });
		const active2 = rule({ id: 'a', state: 'ACTIVE', priority: 1 });
		const inReview = rule({ id: 'c', state: 'IN_REVIEW' });
		const revoked = rule({ id: 'd', state: 'REVOKED' });
		const rejected = rule({ id: 'e', state: 'REJECTED' });
		const groups = groupRulesByState([active1, inReview, active2, revoked, rejected]);
		expect(groups.active).toEqual([active2, active1]);
		expect(groups.inReview).toEqual([inReview]);
		expect(groups.revoked).toEqual([revoked]);
		expect(groups.rejected).toEqual([rejected]);
	});
});

describe('sortByRecentlyCreated', () => {
	it('sorts newest first', () => {
		const older = rule({ id: 'a', createdAt: '2024-01-01T00:00:00Z' });
		const newer = rule({ id: 'b', createdAt: '2024-06-01T00:00:00Z' });
		expect(sortByRecentlyCreated([older, newer])).toEqual([newer, older]);
	});
});

function category(overrides: Partial<Category> & { id: string }): Category {
	return {
		slug: overrides.id,
		name: overrides.id,
		kind: 'EXPENSE',
		parentId: null,
		depth: 1,
		archived: false,
		origin: 'seed',
		...overrides
	};
}

describe('buildCategoryTree', () => {
	it('nests children under their parent and sorts siblings by name', () => {
		const food = category({ id: '1', slug: 'food', name: 'Food', depth: 1 });
		const groceries = category({
			id: '2',
			slug: 'food.groceries',
			name: 'Groceries',
			depth: 2,
			parentId: '1'
		});
		const dining = category({
			id: '3',
			slug: 'food.dining',
			name: 'Dining',
			depth: 2,
			parentId: '1'
		});

		const tree = buildCategoryTree([food, groceries, dining]);
		expect(tree).toHaveLength(1);
		expect(tree[0].id).toBe('1');
		expect(tree[0].children.map((c) => c.name)).toEqual(['Dining', 'Groceries']);
	});

	it('treats an unknown parentId as a root (defensive against a stale projection)', () => {
		const orphan = category({ id: '1', parentId: 'missing' });
		expect(buildCategoryTree([orphan])).toEqual([{ ...orphan, children: [] }]);
	});
});

describe('canHaveChildren / MAX_CATEGORY_DEPTH', () => {
	it('refuses children at the depth-3 cap', () => {
		expect(MAX_CATEGORY_DEPTH).toBe(3);
		expect(canHaveChildren({ depth: 2 })).toBe(true);
		expect(canHaveChildren({ depth: 3 })).toBe(false);
	});
});

describe('flattenCategoryTreeWithPath', () => {
	it('joins ancestor names with " / "', () => {
		const food = category({ id: '1', name: 'Food', depth: 1 });
		const groceries = category({ id: '2', name: 'Groceries', depth: 2, parentId: '1' });
		const tree = buildCategoryTree([food, groceries]);
		const flat = flattenCategoryTreeWithPath(tree);
		expect(flat.map((f) => f.path)).toEqual(['Food', 'Food / Groceries']);
	});
});

describe('camelCase keys are not conditions', () => {
	it('ignores the old camelCase spelling the backend would reject', () => {
		expect(specificityScore({ counterpartyKey: 'lidl' })).toBe(0);
		expect(summarizeConditions({ counterpartyKey: 'lidl' })).toEqual([]);
	});

	it('scores and summarizes account_ids and numeric amounts', () => {
		const c = { account_ids: ['a', 'b'], amount_min: -80, amount_max: 0 };
		expect(specificityScore(c)).toBe(2);
		expect(summarizeConditions(c)).toEqual(['amount -80 – 0', 'account in [2]']);
	});
});

describe('mergeConditions', () => {
	const blank = {
		counterparty_key: '',
		counterparty_iban: '',
		description_contains: '',
		description_regex: '',
		direction: '' as const,
		amount_min: '',
		amount_max: ''
	};

	it('creates snake_case keys, trimmed, omitting empty fields', () => {
		expect(mergeConditions({}, { ...blank, counterparty_key: ' lidl ', amount_min: '-5' })).toEqual(
			{ counterparty_key: 'lidl', amount_min: '-5' }
		);
	});

	it('carries account_ids through an edit unchanged', () => {
		const existing = { counterparty_key: 'lidl', account_ids: ['a1', 'a2'] };
		expect(mergeConditions(existing, { ...blank, counterparty_key: 'rewe' })).toEqual({
			counterparty_key: 'rewe',
			account_ids: ['a1', 'a2']
		});
	});

	it('removes a condition the user cleared', () => {
		expect(
			mergeConditions(
				{ counterparty_key: 'lidl', direction: 'SPENDING' },
				{ ...blank, direction: 'SPENDING' }
			)
		).toEqual({ direction: 'SPENDING' });
	});

	it('does not mutate the stored conditions', () => {
		const existing = { counterparty_key: 'lidl' };
		mergeConditions(existing, blank);
		expect(existing).toEqual({ counterparty_key: 'lidl' });
	});

	it('only ever emits keys the backend accepts', () => {
		const allowed = [
			'counterparty_key',
			'counterparty_iban',
			'description_regex',
			'description_contains',
			'direction',
			'amount_min',
			'amount_max',
			'account_ids'
		];
		const out = mergeConditions(
			{ account_ids: ['x'] },
			{
				counterparty_key: 'a',
				counterparty_iban: 'b',
				description_contains: 'c',
				description_regex: 'd',
				direction: 'INCOME',
				amount_min: '1',
				amount_max: '2'
			}
		);
		expect(Object.keys(out).every((k) => allowed.includes(k))).toBe(true);
	});
});

describe('describeReach', () => {
	it('spells out the zero state', () => {
		expect(describeReach(0)).toBe('matches nothing');
		expect(describeReach(1)).toBe('1 transaction');
		expect(describeReach(37)).toBe('37 transactions');
	});
});
