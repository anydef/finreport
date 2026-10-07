import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { maxBarAmount, shapeCategoryBreakdown } from './breakdownShaping';
import type { CategoryBreakdown } from './graphql/types';

function loadMock(): { data: { categoryBreakdown: CategoryBreakdown } } {
	const file = path.resolve(__dirname, 'graphql/mocks/category-breakdown.json');
	return JSON.parse(readFileSync(file, 'utf-8'));
}

describe('shapeCategoryBreakdown', () => {
	const breakdown = loadMock().data.categoryBreakdown;

	it('sorts rows by amount, largest first', () => {
		const bars = shapeCategoryBreakdown(breakdown);
		const categoryBars = bars.filter((b) => b.kind === 'category');
		expect(categoryBars.map((b) => b.slug)).toEqual(['food', 'transportation']);
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
