import { describe, expect, it } from 'vitest';
import {
	comparisonWindow,
	defaultPair,
	delta,
	formatDeltaPercent,
	parseSpan,
	periodIndexByStart,
	shapeComparison,
	trendBars,
	transactionsHref,
	type CategoryComparison
} from './comparisonView';

const cat = (slug: string) => ({ id: slug, slug, name: slug.toUpperCase(), kind: 'EXPENSE' });
const cell = (amount: string, transactionCount = 1) => ({ amount, transactionCount });
const gone = { amount: '0', transactionCount: 0 };

function period(start: string, end: string, total: string, uncategorized = '0', needsReview = '0') {
	return { start, end, total, uncategorized, needsReview };
}

/** Three months: food steady-ish, gym disappears, travel appears, health fully refunded. */
const data: CategoryComparison = {
	currency: 'EUR',
	periods: [
		period('2026-07-01', '2026-07-31', '600'),
		period('2026-08-01', '2026-08-31', '530', '10', '5'),
		period('2026-09-01', '2026-09-30', '700', '20', '40')
	],
	categories: [
		{
			category: cat('food'),
			total: '1000',
			cells: [cell('300'), cell('330'), cell('370')]
		},
		{
			category: cat('travel'),
			total: '300',
			cells: [gone, gone, cell('300')]
		},
		{
			category: cat('gym'),
			total: '60',
			cells: [cell('30'), cell('30'), gone]
		},
		{
			category: cat('health'),
			total: '0',
			cells: [cell('0', 2), cell('0', 2), cell('0', 2)]
		}
	]
};

describe('delta', () => {
	it('computes absolute and relative change', () => {
		expect(delta(200, 250)).toEqual({ abs: 50, rel: 0.25, kind: 'up' });
		expect(delta(200, 150)).toEqual({ abs: -50, rel: -0.25, kind: 'down' });
	});

	it('flags a category with no base as new, without a percentage', () => {
		expect(delta(0, 80)).toEqual({ abs: 80, rel: null, kind: 'new' });
	});

	it('flags a vanished category as gone, not as -100%', () => {
		expect(delta(80, 0)).toEqual({ abs: -80, rel: null, kind: 'gone' });
	});

	it('never divides by zero when both sides are zero', () => {
		expect(delta(0, 0)).toEqual({ abs: 0, rel: null, kind: 'none' });
	});

	it('treats sub-cent differences as flat', () => {
		expect(delta(10, 10.001).kind).toBe('flat');
	});
});

describe('formatDeltaPercent', () => {
	it('prints signed percentages with a real minus', () => {
		expect(formatDeltaPercent(delta(200, 250))).toBe('+25%');
		expect(formatDeltaPercent(delta(200, 150))).toBe('−25%');
		expect(formatDeltaPercent(delta(3, 4))).toBe('+33.3%');
	});

	it('prints words or a dash where a percentage would mislead', () => {
		expect(formatDeltaPercent(delta(0, 5))).toBe('new');
		expect(formatDeltaPercent(delta(5, 0))).toBe('gone');
		expect(formatDeltaPercent(delta(0, 0))).toBe('–');
		expect(formatDeltaPercent(delta(10, 10))).toBe('0%');
	});
});

describe('shapeComparison', () => {
	const view = shapeComparison(data, 1, 2, true);

	it('diffs the chosen pair of periods in total', () => {
		expect(view.total.base).toBe(530);
		expect(view.total.current).toBe(700);
		expect(view.total.delta.abs).toBe(170);
		expect(view.total.delta.rel).toBeCloseTo(170 / 530);
	});

	it('orders categories by the size of their change', () => {
		// travel +300, gym -30, food +40, health 0
		expect(view.rows.map((r) => r.slug)).toEqual(['travel', 'food', 'gym', 'health']);
	});

	it('marks the appearing and the disappearing category', () => {
		const by = (slug: string) => view.rows.find((r) => r.slug === slug)!;
		expect(by('travel').delta.kind).toBe('new');
		expect(by('gym').delta.kind).toBe('gone');
		expect(by('food').delta.kind).toBe('up');
	});

	it('shows a fully refunded category as present but not changed', () => {
		const health = view.rows.find((r) => r.slug === 'health')!;
		expect(health.cells).toEqual([0, 0, 0]);
		expect(health.delta.kind).toBe('none');
	});

	it('keeps held-for-review beside the total, never inside it', () => {
		expect(view.needsReview.base).toBe(5);
		expect(view.needsReview.current).toBe(40);
		expect(view.total.current).toBe(700);
		expect(view.uncategorized.delta.abs).toBe(10);
	});

	it('flags only the final period as partial, and only when asked', () => {
		expect(view.periods.map((p) => p.partial)).toEqual([false, false, true]);
		expect(shapeComparison(data, 1, 2, false).periods.some((p) => p.partial)).toBe(false);
	});

	it('labels periods with month and year', () => {
		expect(view.periods[2].label).toBe('September 2026');
	});

	it('supports comparing non-adjacent periods', () => {
		const v = shapeComparison(data, 0, 2, false);
		expect(v.rows.find((r) => r.slug === 'food')!.delta.abs).toBe(70);
	});

	it('survives an empty response and out-of-range indexes', () => {
		const empty = shapeComparison({ periods: [], categories: [], currency: 'EUR' }, 3, 5, false);
		expect(empty.total.delta.kind).toBe('none');
		expect(empty.rows).toEqual([]);
		const clamped = shapeComparison(data, -4, 99, false);
		expect(clamped.baseIndex).toBe(0);
		expect(clamped.currentIndex).toBe(2);
	});
});

describe('defaultPair / periodIndexByStart', () => {
	it('defaults to the last two periods', () => {
		expect(defaultPair(6)).toEqual({ base: 4, current: 5 });
		expect(defaultPair(1)).toEqual({ base: 0, current: 0 });
		expect(defaultPair(0)).toEqual({ base: 0, current: 0 });
	});

	it('skips an unfinished last month so the default is not a fake drop', () => {
		expect(defaultPair(6, true)).toEqual({ base: 3, current: 4 });
		expect(defaultPair(2, true)).toEqual({ base: 0, current: 1 });
	});

	it('finds a period by its start date', () => {
		expect(periodIndexByStart(data.periods, '2026-08-01')).toBe(1);
		expect(periodIndexByStart(data.periods, '2026-08-02')).toBeNull();
		expect(periodIndexByStart(data.periods, null)).toBeNull();
	});
});

describe('parseSpan', () => {
	it('accepts the offered spans only', () => {
		expect(parseSpan('3')).toBe(3);
		expect(parseSpan('12')).toBe(12);
		expect(parseSpan('7')).toBe(6);
		expect(parseSpan(null)).toBe(6);
		expect(parseSpan('abc')).toBe(6);
	});
});

describe('comparisonWindow', () => {
	const today = new Date(2026, 9, 9); // 9 Oct 2026

	it('ends today in the current month and flags it partial', () => {
		const w = comparisonWindow(6, null, today);
		expect(w.range).toEqual({ start: '2026-05-01', end: '2026-10-09' });
		expect(w.partial).toBe(true);
		expect(w.endMonth).toBe('2026-10');
	});

	it('ends on the last day of a past month, February included', () => {
		expect(comparisonWindow(3, '2026-09', today).range).toEqual({
			start: '2026-07-01',
			end: '2026-09-30'
		});
		expect(comparisonWindow(3, '2024-02', today).range).toEqual({
			start: '2023-12-01',
			end: '2024-02-29'
		});
		expect(comparisonWindow(3, '2026-09', today).partial).toBe(false);
	});

	it('reaches back across a year boundary', () => {
		expect(comparisonWindow(12, '2026-03', today).range.start).toBe('2025-04-01');
		expect(comparisonWindow(6, null, new Date(2026, 1, 3)).range.start).toBe('2025-09-01');
	});

	it('falls back to the current month for a malformed or future end month', () => {
		expect(comparisonWindow(3, 'junk', today).endMonth).toBe('2026-10');
		expect(comparisonWindow(3, '2026-13', today).endMonth).toBe('2026-10');
		expect(comparisonWindow(3, '2027-01', today).endMonth).toBe('2026-10');
		expect(comparisonWindow(3, '2026-10', today).partial).toBe(true);
	});
});

describe('transactionsHref', () => {
	it('uses the month preset for a whole calendar month', () => {
		expect(
			transactionsHref({ start: '2026-09-01', end: '2026-09-30' }, '2026-10-09', { slug: 'food' })
		).toBe('/transactions?preset=month%3A2026-09&categorySlugs=food');
	});

	it('uses this-month for the unfinished current month', () => {
		expect(
			transactionsHref({ start: '2026-10-01', end: '2026-10-09' }, '2026-10-09', { slug: 'food' })
		).toBe('/transactions?preset=this-month&categorySlugs=food');
	});

	it('falls back to explicit dates for a clipped range', () => {
		const href = transactionsHref({ start: '2026-09-10', end: '2026-09-30' }, '2026-10-09');
		expect(href).toBe('/transactions?preset=custom&start=2026-09-10&end=2026-09-30');
	});

	it('can target the uncategorised and held buckets', () => {
		const p = { start: '2026-02-01', end: '2026-02-28' };
		expect(transactionsHref(p, '2026-10-09', { uncategorized: true })).toContain(
			'uncategorized=true'
		);
		expect(transactionsHref(p, '2026-10-09', { needsReview: true })).toContain('needsReview=true');
	});
});

describe('trendBars', () => {
	it('puts each total in exactly one series and marks the unfinished month', () => {
		const bars = trendBars(shapeComparison(data, 0, 2, true));
		expect(bars.map((b) => [b.other, b.base, b.current])).toEqual([
			[0, 600, 0],
			[530, 0, 0],
			[0, 0, 700]
		]);
		expect(bars[2].label).toBe('Sep 2026*');
		expect(bars[0].label).toBe('Jul 2026');
	});

	it('does not draw a bar twice when base and current are the same period', () => {
		const bars = trendBars(shapeComparison(data, 1, 1, false));
		expect(bars[1]).toMatchObject({ base: 0, current: 530, other: 0 });
	});
});
