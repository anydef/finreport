import { describe, expect, it } from 'vitest';
import counterpartyMock from './graphql/mocks/counterparty-breakdown.json';
import {
	amountTone,
	counterpartyHref,
	isDrillable,
	parseBreakdownDimension,
	parseCounterpartyKeys,
	shapeCounterpartyBreakdown,
	transactionCountLabel,
	writeBreakdownDimension,
	type DrillScope
} from './counterpartyView';
import type { CounterpartyBreakdown } from './graphql/types';
import { clearedFilters } from './transactionFilters';

const scope: DrillScope = {
	preset: 'this-month',
	start: '2026-10-01',
	end: '2026-10-31',
	panel: clearedFilters()
};

function breakdown(overrides: Partial<CounterpartyBreakdown> = {}): CounterpartyBreakdown {
	return {
		rows: [
			{ counterpartyKey: 'pay', displayName: 'Pay Co', amount: '100.00', transactionCount: 1 },
			{ counterpartyKey: 'shop', displayName: 'Shop', amount: '-45.5', transactionCount: 3 },
			{ counterpartyKey: 'net', displayName: 'Net Zero', amount: '0.00', transactionCount: 2 },
			{ counterpartyKey: null, displayName: 'No merchant key', amount: '-5', transactionCount: 4 }
		],
		other: { counterpartyKey: null, displayName: 'Other', amount: '-0.80', transactionCount: 17 },
		currency: 'EUR',
		...overrides
	};
}

describe('shapeCounterpartyBreakdown', () => {
	const rows = shapeCounterpartyBreakdown(breakdown());

	it('keeps positive nets positive and shows a plus sign', () => {
		expect(rows[0]).toMatchObject({ amount: 100, amountText: '+100.00 EUR', tone: 'income' });
	});

	it('keeps negative nets negative (no Math.abs) with a minus sign', () => {
		expect(rows[1]).toMatchObject({ amount: -45.5, amountText: '−45.50 EUR', tone: 'spending' });
	});

	it('shows a zero net unsigned and neutral', () => {
		expect(rows[2]).toMatchObject({ amount: 0, amountText: '0.00 EUR', tone: 'neutral' });
		expect(rows[2].transactionCount).toBe(2);
	});

	it('marks the null-key row as no-key and not drillable', () => {
		expect(rows[3]).toMatchObject({
			kind: 'no-key',
			counterpartyKey: null,
			label: 'No merchant key',
			drillable: false
		});
	});

	it('puts the other row last, distinct, with its count, and not drillable', () => {
		const other = rows[rows.length - 1];
		expect(other).toMatchObject({
			kind: 'other',
			counterpartyKey: null,
			drillable: false,
			transactionCount: 17
		});
		expect(other.key).toBe('other');
	});

	it('has unique keys', () => {
		expect(new Set(rows.map((r) => r.key)).size).toBe(rows.length);
	});

	it('is empty for no rows and a null other', () => {
		expect(shapeCounterpartyBreakdown(breakdown({ rows: [], other: null }))).toEqual([]);
	});

	it('has no row without other when it is null', () => {
		const shaped = shapeCounterpartyBreakdown(breakdown({ other: null }));
		expect(shaped.some((r) => r.kind === 'other')).toBe(false);
	});

	it('never carries a share', () => {
		expect(Object.keys(rows[0])).not.toContain('share');
	});

	it('shapes the mock fixture: mixed signs, a null key and an other row', () => {
		const shaped = shapeCounterpartyBreakdown(counterpartyMock.data.counterpartyBreakdown);
		expect(shaped.some((r) => r.tone === 'income')).toBe(true);
		expect(shaped.some((r) => r.tone === 'spending')).toBe(true);
		expect(shaped.some((r) => r.kind === 'no-key')).toBe(true);
		expect(shaped[shaped.length - 1].kind).toBe('other');
	});
});

describe('amountTone', () => {
	it('splits by sign', () => {
		expect(amountTone(1)).toBe('income');
		expect(amountTone(-1)).toBe('spending');
		expect(amountTone(0)).toBe('neutral');
	});
});

describe('isDrillable', () => {
	it('is true only for a keyed merchant row', () => {
		expect(isDrillable({ kind: 'merchant', counterpartyKey: 'shop' })).toBe(true);
		expect(isDrillable({ kind: 'no-key', counterpartyKey: null })).toBe(false);
		expect(isDrillable({ kind: 'other', counterpartyKey: null })).toBe(false);
		expect(isDrillable({ kind: 'other', counterpartyKey: 'shop' })).toBe(false);
	});
});

describe('counterpartyHref', () => {
	const [pay, , , noKey, other] = shapeCounterpartyBreakdown(breakdown());

	it('links a keyed row to /transactions with counterpartyKeys', () => {
		expect(counterpartyHref(pay, scope)).toBe('/transactions?counterpartyKeys=pay');
	});

	it('returns null for the no-key and other rows', () => {
		expect(counterpartyHref(noKey, scope)).toBeNull();
		expect(counterpartyHref(other, scope)).toBeNull();
	});

	it('keeps the period and the filter panel', () => {
		const href = counterpartyHref(pay, {
			preset: 'custom',
			start: '2026-01-01',
			end: '2026-03-31',
			panel: { ...clearedFilters(), tags: ['trip'], needsReview: false }
		});
		const url = new URL(href!, 'http://x');
		expect(url.pathname).toBe('/transactions');
		expect(url.searchParams.get('preset')).toBe('custom');
		expect(url.searchParams.get('start')).toBe('2026-01-01');
		expect(url.searchParams.get('end')).toBe('2026-03-31');
		expect(url.searchParams.get('tags')).toBe('trip');
		expect(url.searchParams.get('needsReview')).toBe('false');
		expect(url.searchParams.get('counterpartyKeys')).toBe('pay');
	});

	it('url-encodes keys', () => {
		const href = counterpartyHref({ kind: 'merchant', counterpartyKey: 'a&b c' }, scope);
		expect(parseCounterpartyKeys(new URL(href!, 'http://x').searchParams)).toEqual(['a&b c']);
	});
});

describe('breakdown dimension param', () => {
	it('defaults to category and only recognises counterparty', () => {
		expect(parseBreakdownDimension(new URLSearchParams())).toBe('category');
		expect(parseBreakdownDimension(new URLSearchParams('breakdown=bogus'))).toBe('category');
		expect(parseBreakdownDimension(new URLSearchParams('breakdown=counterparty'))).toBe(
			'counterparty'
		);
	});

	it('writes counterparty, drops the default, and leaves other params alone', () => {
		const base = new URLSearchParams('preset=year:2026');
		expect(writeBreakdownDimension('counterparty', base).toString()).toBe(
			'preset=year%3A2026&breakdown=counterparty'
		);
		expect(
			writeBreakdownDimension(
				'category',
				new URLSearchParams('breakdown=counterparty&a=1')
			).toString()
		).toBe('a=1');
	});
});

describe('transactionCountLabel', () => {
	it('pluralises', () => {
		expect(transactionCountLabel(1)).toBe('1 transaction');
		expect(transactionCountLabel(0)).toBe('0 transactions');
		expect(transactionCountLabel(17)).toBe('17 transactions');
	});
});
