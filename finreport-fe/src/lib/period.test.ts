import { describe, expect, it } from 'vitest';
import {
	bucketLabel,
	daysBetween,
	defaultGranularity,
	monthLabel,
	monthRange,
	monthSelection,
	parseDateInputValue,
	parsePeriodSelection,
	rangeFromParams,
	selectableMonths,
	selectableYears,
	selectionLabel,
	selectionRange,
	yearRange,
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

describe('monthRange', () => {
	it('covers a 30- and a 31-day month', () => {
		expect(monthRange(2026, 9)).toEqual({ start: '2026-09-01', end: '2026-09-30' });
		expect(monthRange(2026, 8)).toEqual({ start: '2026-08-01', end: '2026-08-31' });
	});

	it('ends February on the 28th, or the 29th in a leap year', () => {
		expect(monthRange(2026, 2).end).toBe('2026-02-28');
		expect(monthRange(2024, 2).end).toBe('2024-02-29');
		expect(monthRange(2100, 2).end).toBe('2100-02-28');
	});

	it('handles December without rolling into the next year', () => {
		expect(monthRange(2025, 12)).toEqual({ start: '2025-12-01', end: '2025-12-31' });
	});
});

describe('yearRange / monthLabel', () => {
	it('spans Jan 1 to Dec 31', () => {
		expect(yearRange(2025)).toEqual({ start: '2025-01-01', end: '2025-12-31' });
	});
	it('labels a month with its full name and year', () => {
		expect(monthLabel(2026, 9)).toBe('September 2026');
		expect(monthLabel(2025, 1)).toBe('January 2025');
	});
});

describe('selectableMonths', () => {
	it('starts at the month before today and runs newest first', () => {
		const months = selectableMonths(new Date(2026, 9, 9), 3);
		expect(months.map((m) => m.id)).toEqual(['month:2026-09', 'month:2026-08', 'month:2026-07']);
		expect(months[0].label).toBe('September 2026');
	});

	it('crosses the year boundary from January', () => {
		const months = selectableMonths(new Date(2026, 0, 15), 3);
		expect(months.map((m) => m.id)).toEqual(['month:2025-12', 'month:2025-11', 'month:2025-10']);
	});

	it('lists 24 distinct months by default, ending 24 months back', () => {
		const months = selectableMonths(new Date(2026, 9, 9));
		expect(months).toHaveLength(24);
		expect(new Set(months.map((m) => m.id)).size).toBe(24);
		expect(months[23].id).toBe('month:2024-10');
	});

	it('is not thrown by a 31st-of-the-month today', () => {
		const months = selectableMonths(new Date(2026, 2, 31), 2);
		expect(months.map((m) => m.id)).toEqual(['month:2026-02', 'month:2026-01']);
	});
});

describe('selectableYears', () => {
	it('lists completed years newest first', () => {
		expect(selectableYears(new Date(2026, 9, 9), 2).map((y) => y.id)).toEqual([
			'year:2025',
			'year:2024'
		]);
	});
});

describe('parsePeriodSelection', () => {
	it('accepts presets, months and years', () => {
		expect(parsePeriodSelection('last-month')).toBe('last-month');
		expect(parsePeriodSelection('custom')).toBe('custom');
		expect(parsePeriodSelection('month:2026-09')).toBe('month:2026-09');
		expect(parsePeriodSelection('year:2025')).toBe('year:2025');
	});

	it('falls back to this-month for missing or malformed input', () => {
		expect(parsePeriodSelection(null)).toBe('this-month');
		expect(parsePeriodSelection('')).toBe('this-month');
		expect(parsePeriodSelection('month:2026-13')).toBe('this-month');
		expect(parsePeriodSelection('month:2026-00')).toBe('this-month');
		expect(parsePeriodSelection('month:26-09')).toBe('this-month');
		expect(parsePeriodSelection('nonsense')).toBe('this-month');
	});

	it('round-trips every selectable month', () => {
		for (const m of selectableMonths(new Date(2026, 9, 9))) {
			expect(parsePeriodSelection(m.id)).toBe(m.id);
			expect(parsePeriodSelection(monthSelection(m.year, m.month))).toBe(m.id);
		}
	});
});

describe('selectionRange / selectionLabel / rangeFromParams', () => {
	const today = new Date(2026, 9, 9);

	it('resolves months and years, and leaves presets unchanged', () => {
		expect(selectionRange('month:2026-09', today)).toEqual({
			start: '2026-09-01',
			end: '2026-09-30'
		});
		expect(selectionRange('year:2025', today)).toEqual({ start: '2025-01-01', end: '2025-12-31' });
		expect(selectionRange('last-month', today)).toEqual(presetRange('last-month', today));
		expect(selectionRange('this-month', today)).toEqual({ start: '2026-10-01', end: '2026-10-09' });
	});

	it('labels each kind of selection', () => {
		expect(selectionLabel('month:2026-09')).toBe('September 2026');
		expect(selectionLabel('year:2025')).toBe('2025');
		expect(selectionLabel('last-3-months')).toBe('Last 3 months');
	});

	it('reads custom start/end from params, defaulting to this month', () => {
		const p = new URLSearchParams('start=2026-01-02&end=2026-01-20');
		expect(rangeFromParams(p, 'custom', today)).toEqual({ start: '2026-01-02', end: '2026-01-20' });
		expect(rangeFromParams(new URLSearchParams(), 'custom', today).start).toBe('2026-10-01');
		expect(rangeFromParams(new URLSearchParams(), 'month:2026-02', today).end).toBe('2026-02-28');
	});
});
