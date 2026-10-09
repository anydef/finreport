import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import {
	childBreakdownFilter,
	childLevel,
	drilldownForBar,
	hasChildCategories,
	maxBarAmount,
	shapeCategoryBreakdown,
	shapeChildBreakdown
} from './breakdownShaping';
import type { Category, CategoryBreakdown } from './graphql/types';

function loadMock(): { data: { categoryBreakdown: CategoryBreakdown } } {
	const file = path.resolve(__dirname, 'graphql/mocks/category-breakdown.json');
	return JSON.parse(readFileSync(file, 'utf-8'));
}

describe('shapeCategoryBreakdown', () => {
	const breakdown = loadMock().data.categoryBreakdown;

	it('sorts rows by amount, largest first', () => {
		const bars = shapeCategoryBreakdown(breakdown);
		const categoryBars = bars.filter((b) => b.kind === 'category');
		expect(categoryBars.map((b) => b.slug)).toEqual(['food', 'transportation', 'personal']);
		expect(categoryBars[0].amount).toBeGreaterThan(categoryBars[1].amount);
	});

	it('appends uncategorized and needs-review last, with a null slug', () => {
		const bars = shapeCategoryBreakdown(breakdown);
		expect(bars.at(-2)).toMatchObject({ kind: 'uncategorized', slug: null });
		expect(bars.at(-1)).toMatchObject({ kind: 'needs-review', slug: null });
	});

	it('omits uncategorized/needs-review when absent', () => {
		const bars = shapeCategoryBreakdown({ ...breakdown, uncategorized: null, needsReview: null });
		expect(bars.every((b) => b.kind === 'category')).toBe(true);
	});

	it('parses the Decimal amount string into a number', () => {
		const bars = shapeCategoryBreakdown(breakdown);
		const food = bars.find((b) => b.slug === 'food')!;
		expect(food.amount).toBe(320.5);
	});
});

describe('maxBarAmount', () => {
	it('returns the largest amount', () => {
		const bars = shapeCategoryBreakdown(loadMock().data.categoryBreakdown);
		expect(maxBarAmount(bars)).toBe(320.5);
	});

	it('returns 0 for an empty list', () => {
		expect(maxBarAmount([])).toBe(0);
	});
});

function loadCategories(): Category[] {
	const file = path.resolve(__dirname, 'graphql/mocks/categories.json');
	return JSON.parse(readFileSync(file, 'utf-8')).data.categories;
}

function loadChildren(): Record<string, CategoryBreakdown> {
	const file = path.resolve(__dirname, 'graphql/mocks/category-breakdown-children.json');
	return JSON.parse(readFileSync(file, 'utf-8')).data;
}

describe('drilldownForBar', () => {
	const bars = shapeCategoryBreakdown(loadMock().data.categoryBreakdown);

	it('narrows a category bar by its slug', () => {
		expect(drilldownForBar(bars.find((b) => b.slug === 'food')!)).toEqual({
			categorySlugs: ['food']
		});
	});

	it('narrows the uncategorized bar to no category and not held', () => {
		expect(drilldownForBar(bars.find((b) => b.kind === 'uncategorized')!)).toEqual({
			uncategorized: true,
			needsReview: false
		});
	});

	it('drills the "(no subcategory)" row into the exact category, not its descendants', () => {
		const children = loadChildren().food;
		const own = shapeChildBreakdown(children, 'food').find((b) => b.own)!;
		expect(drilldownForBar(own)).toEqual({ categorySlugsExact: ['food'] });
		const sibling = shapeChildBreakdown(children, 'food').find((b) => b.slug === 'food.groceries')!;
		expect(drilldownForBar(sibling)).toEqual({ categorySlugs: ['food.groceries'] });
	});

	it('narrows the needs-review bar to held labels', () => {
		expect(drilldownForBar(bars.find((b) => b.kind === 'needs-review')!)).toEqual({
			needsReview: true
		});
	});
});

describe('category children', () => {
	const categories = loadCategories();

	it('knows which categories can expand', () => {
		expect(hasChildCategories('food', categories)).toBe(true);
		expect(hasChildCategories('personal.gym', categories)).toBe(true);
		expect(hasChildCategories('personal.gym.membership', categories)).toBe(false);
		expect(hasChildCategories('nope', categories)).toBe(false);
	});

	it('does not offer an expander for a category whose children are all archived', () => {
		const archived = categories.map((c) => (c.parentId ? { ...c, archived: true } : c));
		expect(hasChildCategories('food', archived)).toBe(false);
	});

	it('asks one level deeper than the category', () => {
		expect(childLevel('food', categories)).toBe(2);
		expect(childLevel('personal.gym', categories)).toBe(3);
	});

	it('scopes the filter to the category', () => {
		expect(childBreakdownFilter({ startDate: '2026-01-01' }, 'food', categories)).toEqual({
			startDate: '2026-01-01',
			categorySlugs: ['food']
		});
	});

	it('intersects with a category restriction already on the filter', () => {
		const scoped = childBreakdownFilter(
			{ categorySlugs: ['food.groceries', 'housing'] },
			'food',
			categories
		);
		expect(scoped.categorySlugs).toEqual(['food.groceries']);
	});

	it('shapes children largest first, with the parent own row last and marked', () => {
		const bars = shapeChildBreakdown(loadChildren().food, 'food');
		expect(bars.map((b) => b.slug)).toEqual([
			'food.groceries',
			'food.restaurants',
			'food.takeaway',
			'food'
		]);
		expect(bars.at(-1)).toMatchObject({ own: true, label: 'Food (no subcategory)' });
		expect(bars[0].own).toBeUndefined();
	});
});
