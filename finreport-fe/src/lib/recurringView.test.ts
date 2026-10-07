import { describe, expect, it } from 'vitest';
import {
	isStale,
	monthlyEquivalent,
	sortByMonthlyEquivalentDesc,
	totalMonthlyEquivalent
} from './recurringView';
import type { RecurringSeries } from './graphql/types';

function series(overrides: Partial<RecurringSeries>): RecurringSeries {
	return {
		id: 'id',
		counterpartyKey: 'key',
		counterpartyName: 'Name',
		direction: 'SPENDING',
		cadence: 'MONTHLY',
		medianAmount: '-39.9000',
		monthlyEquivalent: '-39.9000',
		occurrenceCount: 4,
		firstDate: '2025-01-01',
		lastDate: '2025-04-01',
		nextExpectedDate: '2025-05-01',
		stale: false,
		...overrides
	};
}

describe('monthlyEquivalent', () => {
	it('is unchanged for a monthly cadence', () => {
		expect(monthlyEquivalent('-39.9000', 'MONTHLY')).toBe('-39.9000');
	});

	it('divides by 3 for a quarterly cadence, exact to 4dp half-up', () => {
		expect(monthlyEquivalent('-187.4300', 'QUARTERLY')).toBe('-62.4767');
	});

	it('divides by 12 for a yearly cadence', () => {
		expect(monthlyEquivalent('1200.0000', 'YEARLY')).toBe('100.0000');
	});

	it('rounds half-up away from zero on negative amounts', () => {
		// -100 / 3 = -33.3333... -> half-up away from zero -> -33.3333
		expect(monthlyEquivalent('-100', 'QUARTERLY')).toBe('-33.3333');
	});
});

describe('totalMonthlyEquivalent', () => {
	it('sums only expense (SPENDING) series as positive magnitudes', () => {
		const rows = [
			series({ direction: 'SPENDING', monthlyEquivalent: '-39.9000' }),
			series({ direction: 'SPENDING', monthlyEquivalent: '-62.4767' }),
			series({ direction: 'INCOME', monthlyEquivalent: '500.0000' })
		];
		expect(totalMonthlyEquivalent(rows)).toBe('102.3767');
	});

	it('is 0.0000 for an empty list', () => {
		expect(totalMonthlyEquivalent([])).toBe('0.0000');
	});
});

describe('sortByMonthlyEquivalentDesc', () => {
	it('sorts by magnitude descending regardless of sign', () => {
		const rows = [
			series({ id: 'a', monthlyEquivalent: '-39.9000' }),
			series({ id: 'b', monthlyEquivalent: '-62.4767' }),
			series({ id: 'c', monthlyEquivalent: '10.0000' })
		];
		expect(sortByMonthlyEquivalentDesc(rows).map((s) => s.id)).toEqual(['b', 'a', 'c']);
	});
});

describe('isStale', () => {
	it('is false when next expected date is in the future', () => {
		expect(isStale('2099-01-01', 'MONTHLY', new Date('2025-01-01T00:00:00Z'))).toBe(false);
	});

	it('is false within the cadence grace window', () => {
		expect(isStale('2025-01-01', 'MONTHLY', new Date('2025-01-05T00:00:00Z'))).toBe(false);
	});

	it('is true once the grace window has passed', () => {
		expect(isStale('2025-01-01', 'MONTHLY', new Date('2025-02-01T00:00:00Z'))).toBe(true);
	});

	it('uses a wider grace window for yearly cadence', () => {
		expect(isStale('2025-01-01', 'YEARLY', new Date('2025-01-20T00:00:00Z'))).toBe(false);
		expect(isStale('2025-01-01', 'YEARLY', new Date('2025-03-01T00:00:00Z'))).toBe(true);
	});
});
