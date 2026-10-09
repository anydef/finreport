import { describe, expect, it } from 'vitest';
import { mockCategoryComparison } from './comparisonMock';
import { delta, shapeComparison } from '$lib/comparisonView';

const today = new Date(2026, 9, 9);
const window = (startDate: string, endDate: string) => ({ filter: { startDate, endDate } });

describe('mockCategoryComparison', () => {
	const out = mockCategoryComparison(window('2026-05-01', '2026-10-09'), today).categoryComparison;

	it('serves one period per month in the requested range, clipped at the ends', () => {
		expect(out.periods).toHaveLength(6);
		expect(out.periods[0]).toMatchObject({ start: '2026-05-01', end: '2026-05-31' });
		expect(out.periods[5]).toMatchObject({ start: '2026-10-01', end: '2026-10-09' });
	});

	it('aligns every category to every period', () => {
		for (const c of out.categories) expect(c.cells).toHaveLength(out.periods.length);
	});

	it('includes an appearing and a disappearing category', () => {
		const view = shapeComparison(out, 4, 5, true);
		const kind = (slug: string) => view.rows.find((r) => r.slug === slug)?.delta.kind;
		expect(kind('travel')).toBe('new');
		expect(kind('gym')).toBe('gone');
	});

	it('has a category that is new in a September-ending view', () => {
		const sept = mockCategoryComparison(window('2026-07-01', '2026-09-30'), today);
		const view = shapeComparison(sept.categoryComparison, 1, 2, false);
		expect(view.rows.find((r) => r.slug === 'courses')?.delta.kind).toBe('new');
	});

	it('nets a fully reimbursed bill to zero while still counting both transactions', () => {
		const aug = mockCategoryComparison(window('2026-08-01', '2026-08-31'), today);
		const health = aug.categoryComparison.categories.find((c) => c.category.slug === 'health')!;
		expect(Number(health.cells[0].amount)).toBe(0);
		expect(health.cells[0].transactionCount).toBe(2);
		expect(delta(0, 0).kind).toBe('none');
	});

	it('keeps held-for-review out of the period total', () => {
		const p = out.periods[4];
		const cats = out.categories.reduce((s, c) => s + Number(c.cells[4].amount), 0);
		expect(Number(p.needsReview)).toBeGreaterThan(0);
		expect(Number(p.total)).toBeCloseTo(cats + Number(p.uncategorized), 2);
	});
});
