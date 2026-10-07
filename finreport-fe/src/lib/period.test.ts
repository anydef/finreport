import { describe, expect, it } from 'vitest';
import {
	bucketLabel,
	daysBetween,
	defaultGranularity,
	parseDateInputValue,
	presetRange,
	toDateInputValue
} from './period';

describe('toDateInputValue / parseDateInputValue', () => {
	it('formats a date as YYYY-MM-DD using local date parts', () => {
		expect(toDateInputValue(new Date(2026, 6, 17))).toBe('2026-07-17');
	});

	it('pads single-digit months and days', () => {
		expect(toDateInputValue(new Date(2026, 0, 5))).toBe('2026-01-05');
	});

	it('round-trips through parseDateInputValue', () => {
		const date = parseDateInputValue('2026-03-02');
		expect(date.getFullYear()).toBe(2026);
		expect(date.getMonth()).toBe(2);
		expect(date.getDate()).toBe(2);
	});
});

describe('daysBetween', () => {
	it('counts a single day as 1', () => {
		expect(daysBetween({ start: '2026-01-01', end: '2026-01-01' })).toBe(1);
	});

	it('counts an inclusive range', () => {
		expect(daysBetween({ start: '2026-01-01', end: '2026-01-31' })).toBe(31);
	});
});

describe('defaultGranularity', () => {
	it('picks DAY for ranges of 31 days or fewer', () => {
		expect(defaultGranularity({ start: '2026-01-01', end: '2026-01-31' })).toBe('DAY');
	});

	it('picks WEEK just above the day threshold', () => {
		expect(defaultGranularity({ start: '2026-01-01', end: '2026-02-01' })).toBe('WEEK');
	});

	it('picks WEEK at the 26-week (182 day) boundary', () => {
		expect(defaultGranularity({ start: '2026-01-01', end: '2026-07-01' })).toBe('WEEK');
	});

	it('picks MONTH beyond 182 days', () => {
		expect(defaultGranularity({ start: '2026-01-01', end: '2026-12-31' })).toBe('MONTH');
	});
});

describe('presetRange', () => {
	const today = new Date(2026, 6, 17); // 2026-07-17

	it('last-30-days: a rolling 30-day window ending today, inclusive', () => {
		expect(presetRange('last-30-days', today)).toEqual({ start: '2026-06-18', end: '2026-07-17' });
	});

	it('last-30-days: crosses a month and a year boundary', () => {
		expect(presetRange('last-30-days', new Date(2026, 0, 5))).toEqual({
			start: '2025-12-07',
			end: '2026-01-05'
		});
	});

	it('last-30-days: is not the same as this-month early in the month', () => {
		const early = new Date(2026, 6, 3); // 2026-07-03
		expect(presetRange('this-month', early)).toEqual({ start: '2026-07-01', end: '2026-07-03' });
		expect(presetRange('last-30-days', early)).toEqual({ start: '2026-06-04', end: '2026-07-03' });
	});

	it('this-month: first of month through today', () => {
		expect(presetRange('this-month', today)).toEqual({ start: '2026-07-01', end: '2026-07-17' });
	});

	it('last-month: full previous calendar month', () => {
		expect(presetRange('last-month', today)).toEqual({ start: '2026-06-01', end: '2026-06-30' });
	});

	it('last-month handles January rolling back to December of the prior year', () => {
		expect(presetRange('last-month', new Date(2026, 0, 15))).toEqual({
			start: '2025-12-01',
			end: '2025-12-31'
		});
	});

	it('last-3-months: first of the month two months back through today', () => {
		expect(presetRange('last-3-months', today)).toEqual({ start: '2026-05-01', end: '2026-07-17' });
	});

	it('this-year: Jan 1 through today', () => {
		expect(presetRange('this-year', today)).toEqual({ start: '2026-01-01', end: '2026-07-17' });
	});
});

describe('bucketLabel', () => {
	it('formats a DAY bucket as "Mon D"', () => {
		expect(bucketLabel({ start: '2026-01-05', end: '2026-01-05' }, 'DAY')).toBe('Jan 5');
	});

	it('formats a WEEK bucket within one month as "Mon D–D"', () => {
		expect(bucketLabel({ start: '2026-01-05', end: '2026-01-11' }, 'WEEK')).toBe('Jan 5\u201311');
	});

	it('formats a WEEK bucket spanning two months', () => {
		expect(bucketLabel({ start: '2026-01-26', end: '2026-02-01' }, 'WEEK')).toBe(
			'Jan 26\u2013Feb 1'
		);
	});

	it('formats a MONTH bucket as "Mon YYYY"', () => {
		expect(bucketLabel({ start: '2026-01-01', end: '2026-01-31' }, 'MONTH')).toBe('Jan 2026');
	});
});
