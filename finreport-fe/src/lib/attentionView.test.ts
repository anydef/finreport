import { describe, expect, it } from 'vitest';
import type { AttentionSummary } from './graphql/types';
import { ALL_TIME_START, attentionItems, formatWorth, uncategorizedHref } from './attentionView';

const summary = (u: [number, string], r: [number, string]): AttentionSummary => ({
	uncategorized: { count: u[0], totalAmount: u[1] },
	needsReview: { count: r[0], totalAmount: r[1] }
});

describe('attentionItems', () => {
	it('is empty when nothing needs attention', () => {
		expect(attentionItems(summary([0, '0'], [0, '0']), 'EUR', '2026-10-08')).toEqual([]);
	});

	it('lists both buckets with counts and worth', () => {
		const items = attentionItems(summary([190, '4120.5'], [196, '880']), 'EUR', '2026-10-08');
		expect(items.map((i) => [i.key, i.count, i.worth])).toEqual([
			['uncategorized', 190, '4,120.50 EUR'],
			['needsReview', 196, '880.00 EUR']
		]);
	});

	it('omits a bucket that is empty', () => {
		const items = attentionItems(summary([0, '0'], [3, '12']), 'EUR', '2026-10-08');
		expect(items.map((i) => i.key)).toEqual(['needsReview']);
		expect(items[0].href).toBe('/review');
	});
});

describe('uncategorizedHref', () => {
	it('opens an all-time /transactions list with the uncategorised filter on', () => {
		const url = new URL(uncategorizedHref('2026-10-08'), 'http://x');
		expect(url.pathname).toBe('/transactions');
		expect(url.searchParams.get('uncategorized')).toBe('true');
		expect(url.searchParams.get('preset')).toBe('custom');
		expect(url.searchParams.get('start')).toBe(ALL_TIME_START);
		expect(url.searchParams.get('end')).toBe('2026-10-08');
	});
});

describe('formatWorth', () => {
	it('is unsigned', () => {
		expect(formatWorth('-12.5', 'EUR')).toBe('12.50 EUR');
	});
});
