import { describe, expect, it } from 'vitest';
import {
	buildCategoryTree,
	canHaveChildren,
	flattenCategoryTreeWithPath,
	groupRulesByState,
	isAutoApproved,
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
		createdAt: '2024-01-01T00:00:00Z',
		...overrides
	};
}

describe('specificityScore', () => {
	it('weighs counterpartyKey and counterpartyIban at 2', () => {
		expect(specificityScore({ counterpartyKey: 'lidl' })).toBe(2);
		expect(specificityScore({ counterpartyIban: 'DE12' })).toBe(2);
	});

	it('weighs a bounded amount range at 1, only when both bounds are present', () => {
		expect(specificityScore({ amountMin: '0' })).toBe(0);
		expect(specificityScore({ amountMin: '0', amountMax: '50' })).toBe(1);
	});

	it('sums every present condition', () => {
		expect(
			specificityScore({
				counterpartyKey: 'lidl',
				amountMin: '0',
				amountMax: '50',
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
		const specific = rule({ id: 'a', conditions: { counterpartyKey: 'lidl' } });
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
			summarizeConditions({ counterpartyKey: 'lidl', direction: 'SPENDING', amountMax: '50' })
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
