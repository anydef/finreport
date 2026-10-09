/**
 * Pure period/date-range math for the dashboard period selector (§6, §9 WP5).
 * No Svelte, no I/O — covered directly by vitest (`period.test.ts`).
 */

export type Granularity = 'DAY' | 'WEEK' | 'MONTH';

export interface DateRange {
	/** Inclusive, "YYYY-MM-DD". */
	start: string;
	/** Inclusive, "YYYY-MM-DD". */
	end: string;
}

export type PeriodPresetId =
	| 'last-30-days'
	| 'this-month'
	| 'last-month'
	| 'last-3-months'
	| 'this-year'
	| 'custom';

export interface PeriodPreset {
	id: PeriodPresetId;
	label: string;
}

export const PERIOD_PRESETS: PeriodPreset[] = [
	{ id: 'last-30-days', label: 'Last 30 days' },
	{ id: 'this-month', label: 'This month' },
	{ id: 'last-month', label: 'Last month' },
	{ id: 'last-3-months', label: 'Last 3 months' },
	{ id: 'this-year', label: 'This year' },
	{ id: 'custom', label: 'Custom range' }
];

/**
 * Format a date as a "YYYY-MM-DD" string using local date parts (not UTC),
 * matching what `<input type="date">` produces/expects.
 */
export function toDateInputValue(date: Date): string {
	const year = date.getFullYear();
	const month = String(date.getMonth() + 1).padStart(2, '0');
	const day = String(date.getDate()).padStart(2, '0');
	return `${year}-${month}-${day}`;
}

/**
 * Parse a "YYYY-MM-DD" string as a LOCAL date (not UTC), avoiding the
 * off-by-one-day shift that `new Date("YYYY-MM-DD")` introduces.
 */
export function parseDateInputValue(value: string): Date {
	const [y, m, d] = value.split('-').map(Number);
	return new Date(y, m - 1, d);
}

/** Inclusive day count between two "YYYY-MM-DD" dates. */
export function daysBetween(range: DateRange): number {
	const start = parseDateInputValue(range.start);
	const end = parseDateInputValue(range.end);
	const ms = end.getTime() - start.getTime();
	return Math.round(ms / (24 * 60 * 60 * 1000)) + 1;
}

/**
 * Default granularity for a range (§6): <= 31 days -> day, <= 26 weeks (182
 * days) -> week, else month.
 */
export function defaultGranularity(range: DateRange): Granularity {
	const days = daysBetween(range);
	if (days <= 31) return 'DAY';
	if (days <= 182) return 'WEEK';
	return 'MONTH';
}

/** Resolve a non-custom preset into a concrete inclusive date range. */
export function presetRange(preset: PeriodPresetId, today: Date): DateRange {
	switch (preset) {
		// Rolling window, deliberately distinct from `this-month`: that preset
		// runs from the 1st to today, which is month-to-date and therefore
		// shorter and shorter the nearer the 1st you are. This one always
		// covers the same span of days. Inclusive of both ends, so the start is
		// 29 days back.
		case 'last-30-days': {
			const start = new Date(today.getFullYear(), today.getMonth(), today.getDate() - 29);
			return { start: toDateInputValue(start), end: toDateInputValue(today) };
		}
		case 'this-month': {
			const start = new Date(today.getFullYear(), today.getMonth(), 1);
			return { start: toDateInputValue(start), end: toDateInputValue(today) };
		}
		case 'last-month': {
			const start = new Date(today.getFullYear(), today.getMonth() - 1, 1);
			const end = new Date(today.getFullYear(), today.getMonth(), 0);
			return { start: toDateInputValue(start), end: toDateInputValue(end) };
		}
		case 'last-3-months': {
			const start = new Date(today.getFullYear(), today.getMonth() - 2, 1);
			return { start: toDateInputValue(start), end: toDateInputValue(today) };
		}
		case 'this-year': {
			const start = new Date(today.getFullYear(), 0, 1);
			return { start: toDateInputValue(start), end: toDateInputValue(today) };
		}
		case 'custom':
			// Custom has no derivable range; caller supplies explicit start/end.
			return { start: toDateInputValue(today), end: toDateInputValue(today) };
	}
}

/**
 * Human label for a dense cashflow bucket (`CashflowBucket.start`/`end`,
 * already "YYYY-MM-DD" from the server), shaped per granularity for chart
 * axis ticks.
 */
export function bucketLabel(
	bucket: { start: string; end: string },
	granularity: Granularity
): string {
	const start = parseDateInputValue(bucket.start);
	const end = parseDateInputValue(bucket.end);
	const monthShort = (d: Date) => d.toLocaleDateString('en-US', { month: 'short' });

	switch (granularity) {
		case 'DAY':
			return `${monthShort(start)} ${start.getDate()}`;
		case 'WEEK':
			if (start.getMonth() === end.getMonth()) {
				return `${monthShort(start)} ${start.getDate()}\u2013${end.getDate()}`;
			}
			return `${monthShort(start)} ${start.getDate()}\u2013${monthShort(end)} ${end.getDate()}`;
		case 'MONTH':
			return start.toLocaleDateString('en-US', { month: 'short', year: 'numeric' });
	}
}

/**
 * Calendar month / year selection. Carried in the same `preset` URL param as
 * the presets above, as `month:YYYY-MM` or `year:YYYY`, so a month view is
 * linkable and survives a reload without any new param to keep in step.
 */
export type MonthSelection = `month:${string}`;
export type YearSelection = `year:${string}`;
export type PeriodSelection = PeriodPresetId | MonthSelection | YearSelection;

export interface SelectableMonth {
	id: MonthSelection;
	label: string;
	year: number;
	/** 1-12. */
	month: number;
}

export interface SelectableYear {
	id: YearSelection;
	label: string;
	year: number;
}

const DEFAULT_SELECTION: PeriodSelection = 'this-month';

export function monthSelection(year: number, month: number): MonthSelection {
	return `month:${String(year).padStart(4, '0')}-${String(month).padStart(2, '0')}`;
}

export function yearSelection(year: number): YearSelection {
	return `year:${String(year).padStart(4, '0')}`;
}

/** Inclusive bounds of a calendar month (`month` is 1-12); handles leap Februaries. */
export function monthRange(year: number, month: number): DateRange {
	return {
		start: toDateInputValue(new Date(year, month - 1, 1)),
		// Day 0 of the next month is the last day of this one.
		end: toDateInputValue(new Date(year, month, 0))
	};
}

export function yearRange(year: number): DateRange {
	return {
		start: toDateInputValue(new Date(year, 0, 1)),
		end: toDateInputValue(new Date(year, 11, 31))
	};
}

/** "September 2026". */
export function monthLabel(year: number, month: number): string {
	return new Date(year, month - 1, 1).toLocaleDateString('en-US', {
		month: 'long',
		year: 'numeric'
	});
}

/**
 * The months worth listing, newest first, starting with the month BEFORE
 * today's (the current month is the `this-month` preset). Crosses year
 * boundaries correctly because the day is pinned to the 1st before `Date`
 * normalises a negative month.
 */
export function selectableMonths(today: Date, count = 24): SelectableMonth[] {
	const months: SelectableMonth[] = [];
	for (let back = 1; back <= count; back++) {
		const d = new Date(today.getFullYear(), today.getMonth() - back, 1);
		const year = d.getFullYear();
		const month = d.getMonth() + 1;
		months.push({ id: monthSelection(year, month), label: monthLabel(year, month), year, month });
	}
	return months;
}

/** Completed calendar years, newest first (the current one is `this-year`). */
export function selectableYears(today: Date, count = 3): SelectableYear[] {
	const years: SelectableYear[] = [];
	for (let back = 1; back <= count; back++) {
		const year = today.getFullYear() - back;
		years.push({ id: yearSelection(year), label: String(year), year });
	}
	return years;
}

const MONTH_RE = /^month:(\d{4})-(\d{2})$/;
const YEAR_RE = /^year:(\d{4})$/;

/**
 * Validate a raw `preset` search param. Anything unrecognised (a hand-edited
 * URL, `month:2026-13`) falls back to the default instead of crashing the load.
 */
export function parsePeriodSelection(raw: string | null | undefined): PeriodSelection {
	if (!raw) return DEFAULT_SELECTION;
	if (PERIOD_PRESETS.some((p) => p.id === raw)) return raw as PeriodPresetId;
	const m = MONTH_RE.exec(raw);
	if (m && Number(m[2]) >= 1 && Number(m[2]) <= 12) return raw as MonthSelection;
	if (YEAR_RE.test(raw)) return raw as YearSelection;
	return DEFAULT_SELECTION;
}

/** The inclusive range for any non-custom selection. */
export function selectionRange(selection: PeriodSelection, today: Date): DateRange {
	const m = MONTH_RE.exec(selection);
	if (m) return monthRange(Number(m[1]), Number(m[2]));
	const y = YEAR_RE.exec(selection);
	if (y) return yearRange(Number(y[1]));
	return presetRange(selection as PeriodPresetId, today);
}

/** Display label for any selection, e.g. for a heading or a select's current value. */
export function selectionLabel(selection: PeriodSelection): string {
	const m = MONTH_RE.exec(selection);
	if (m) return monthLabel(Number(m[1]), Number(m[2]));
	const y = YEAR_RE.exec(selection);
	if (y) return y[1];
	return PERIOD_PRESETS.find((p) => p.id === selection)?.label ?? selection;
}

/**
 * The range a page loads for a URL: the selection's own range, or for
 * `custom` the explicit `start`/`end` params (defaulting to this month).
 */
export function rangeFromParams(
	params: URLSearchParams,
	selection: PeriodSelection,
	today: Date
): DateRange {
	if (selection === 'custom') {
		const fallback = presetRange('this-month', today);
		return {
			start: params.get('start') ?? fallback.start,
			end: params.get('end') ?? fallback.end
		};
	}
	return selectionRange(selection, today);
}
