import { describe, expect, it } from 'vitest';
import {
	activeCategories,
	buildCategoryTree,
	descendantSlugs,
	expandSelectedSlugs,
	filterCategoryTree,
	filterOptionGroups,
	flattenCategoryTree
} from './categoryTree';
import type { Category } from './graphql/types';

function cat(partial: Partial<Category> & Pick<Category, 'id' | 'slug' | 'name'>): Category {
	return {
		kind: 'EXPENSE',
		parentId: null,
		depth: 1,
		archived: false,
		origin: 'seed',
		...partial
	};
}

const food = cat({ id: '1', slug: 'food', name: 'Food' });
const groceries = cat({
	id: '2',
	slug: 'food.groceries',
	name: 'Groceries',
	parentId: '1',
	depth: 2
});
const bakery = cat({
	id: '3',
	slug: 'food.groceries.bakery',
	name: 'Bakery',
	parentId: '2',
	depth: 3
});
const transport = cat({ id: '4', slug: 'transportation', name: 'Transportation' });
const archived = cat({ id: '5', slug: 'old', name: 'Old', archived: true });

const categories = [food, groceries, bakery, transport, archived];

describe('buildCategoryTree', () => {
	it('nests children under their parent, sorted by name', () => {
		const tree = buildCategoryTree(categories);
		expect(tree.map((n) => n.slug)).toEqual(['food', 'old', 'transportation']);
		const foodNode = tree.find((n) => n.slug === 'food')!;
		expect(foodNode.children.map((n) => n.slug)).toEqual(['food.groceries']);
		expect(foodNode.children[0].children.map((n) => n.slug)).toEqual(['food.groceries.bakery']);
	});

	it('returns an empty forest for an empty list', () => {
		expect(buildCategoryTree([])).toEqual([]);
	});
});

describe('flattenCategoryTree', () => {
	it('depth-first flattens a forest back to a flat list', () => {
		const flat = flattenCategoryTree(buildCategoryTree(categories));
		expect(flat.map((n) => n.slug)).toEqual([
			'food',
			'food.groceries',
			'food.groceries.bakery',
			'old',
			'transportation'
		]);
	});
});

describe('descendantSlugs', () => {
	it('includes the slug itself and every descendant', () => {
		expect(descendantSlugs('food', categories).sort()).toEqual(
			['food', 'food.groceries', 'food.groceries.bakery'].sort()
		);
	});

	it('returns just the slug for a leaf', () => {
		expect(descendantSlugs('transportation', categories)).toEqual(['transportation']);
	});

	it('returns just the slug for an unknown slug', () => {
		expect(descendantSlugs('nope', categories)).toEqual(['nope']);
	});
});

describe('expandSelectedSlugs', () => {
	it('expands a parent selection into itself plus descendants, de-duplicated', () => {
		const expanded = expandSelectedSlugs(['food', 'food.groceries'], categories).sort();
		expect(expanded).toEqual(['food', 'food.groceries', 'food.groceries.bakery'].sort());
	});

	it('returns an empty array for an empty selection', () => {
		expect(expandSelectedSlugs([], categories)).toEqual([]);
	});
});

describe('activeCategories', () => {
	it('excludes archived categories', () => {
		expect(activeCategories(categories).map((c) => c.slug)).not.toContain('old');
	});
});

describe('filterCategoryTree', () => {
	const forest = buildCategoryTree(activeCategories(categories));
	const slugs = (f: ReturnType<typeof buildCategoryTree>) =>
		flattenCategoryTree(f).map((n) => n.slug);

	it('returns everything for an empty or blank query', () => {
		expect(filterCategoryTree(forest, '')).toBe(forest);
		expect(filterCategoryTree(forest, '   ')).toBe(forest);
	});

	it('matches directly on name', () => {
		expect(slugs(filterCategoryTree(forest, 'transport'))).toEqual(['transportation']);
	});

	it('matches on slug', () => {
		expect(slugs(filterCategoryTree(forest, 'groceries.bakery'))).toEqual([
			'food',
			'food.groceries',
			'food.groceries.bakery'
		]);
	});

	it('keeps ancestors of a matching descendant and drops unrelated branches', () => {
		const result = filterCategoryTree(forest, 'bakery');
		expect(slugs(result)).toEqual(['food', 'food.groceries', 'food.groceries.bakery']);
		expect(slugs(result)).not.toContain('transportation');
	});

	it('is case-insensitive', () => {
		expect(slugs(filterCategoryTree(forest, 'BAKERY'))).toContain('food.groceries.bakery');
	});

	it('returns an empty forest when nothing matches', () => {
		expect(filterCategoryTree(forest, 'zzz')).toEqual([]);
	});
});

describe('filterOptionGroups', () => {
	const groups = [
		{
			groupLabel: 'Food',
			options: [
				{ slug: 'food-groceries', label: 'Food / Groceries' },
				{ slug: 'food-dining', label: 'Food / Dining out' }
			]
		},
		{ groupLabel: 'Transport', options: [{ slug: 'car-fuel', label: 'Transport / Fuel' }] }
	];

	it('matches on the label', () => {
		const out = filterOptionGroups(groups, 'dining');
		expect(out).toEqual([{ groupLabel: 'Food', options: [groups[0].options[1]] }]);
	});

	it('matches on the slug', () => {
		const out = filterOptionGroups(groups, 'car-fuel');
		expect(out.map((g) => g.groupLabel)).toEqual(['Transport']);
	});

	it('drops a group with no matching option', () => {
		expect(filterOptionGroups(groups, 'fuel').map((g) => g.groupLabel)).toEqual(['Transport']);
	});

	it('keeps a group when one option matches', () => {
		const out = filterOptionGroups(groups, 'groc');
		expect(out).toHaveLength(1);
		expect(out[0].options.map((o) => o.slug)).toEqual(['food-groceries']);
	});

	it('is case-insensitive and trims the query', () => {
		expect(filterOptionGroups(groups, '  GROCERIES ')[0].options[0].slug).toBe('food-groceries');
	});

	it('returns everything for a blank query', () => {
		expect(filterOptionGroups(groups, '')).toEqual(groups);
		expect(filterOptionGroups(groups, '   ')).toEqual(groups);
	});

	it('returns empty when nothing matches', () => {
		expect(filterOptionGroups(groups, 'zzz')).toEqual([]);
	});
});
