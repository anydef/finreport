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
