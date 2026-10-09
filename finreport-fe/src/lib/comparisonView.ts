/**
 * Pure shaping for the month-comparison page (`/compare`): the window of
 * months to ask the backend for, and the `categoryComparison` response turned
 * into "what changed between period A and period B" rows. No Svelte, no I/O;
 * covered by `comparisonView.test.ts`.
 */

import { decimalToNumber } from './chartShaping';
import {
	monthLabel,
	monthRange,
	monthSelection,
	parseDateInputValue,
	selectableMonths,
	toDateInputValue,
	type DateRange,
	type SelectableMonth
} from './period';

// ---------------------------------------------------------------- response

export interface ComparisonCell {
	/** Magnitude of the category's signed net in the period. */
	amount: string;
	/** 0 = absent from the period altogether. */
	transactionCount: number;
}

export interface ComparisonPeriod {
	start: string;
	end: string;
	/** Categories plus `uncategorized`; never the held-for-review amount. */
	total: string;
	uncategorized: string;
	needsReview: string;
}

export interface ComparisonCategory {
	category: { id: string; slug: string; name: string; kind: string };
	total: string;
	/** One per period, same order. */
	cells: ComparisonCell[];
}

export interface CategoryComparison {
	periods: ComparisonPeriod[];
	categories: ComparisonCategory[];
	currency: string;
}

// ------------------------------------------------------------------ window

export const SPAN_OPTIONS = [3, 6, 12] as const;
export const DEFAULT_SPAN = 6;

/** Months to compare; anything unrecognised falls back to the default. */
export function parseSpan(raw: string | null | undefined): number {
	const n = Number(raw);
	return (SPAN_OPTIONS as readonly number[]).includes(n) ? n : DEFAULT_SPAN;
}

export interface ComparisonWindow {
	/** What to send as the filter's `startDate`/`endDate`. */
	range: DateRange;
	/** The last period is the current month, cut off at today. */
	partial: boolean;
	/** "YYYY-MM" of the final month. */
	endMonth: string;
}

function monthKey(year: number, month: number): string {
	return monthSelection(year, month).slice('month:'.length);
}

/**
 * The `months` calendar months ending at `endMonth` ("YYYY-MM"). A missing,
 * malformed or not-yet-started end month means the current one, which is cut
 * off at today so its half-finished total is not mistaken for a drop.
 */
export function comparisonWindow(
	months: number,
	endMonth: string | null | undefined,
	today: Date
): ComparisonWindow {
	const currentKey = monthKey(today.getFullYear(), today.getMonth() + 1);
	const valid = /^(\d{4})-(0[1-9]|1[0-2])$/.test(endMonth ?? '') && endMonth! < currentKey;
	const key = valid ? endMonth! : currentKey;
	const [endYear, endMo] = key.split('-').map(Number);

	// Day 1 pinned, so `Date` normalises the negative month across years.
	const first = new Date(endYear, endMo - 1 - (months - 1), 1);
	const start = toDateInputValue(first);
	const partial = key === currentKey;
	const end = partial ? toDateInputValue(today) : monthRange(endYear, endMo).end;
	return { range: { start, end }, partial, endMonth: key };
}

/** The "ending in" choices: the current month first, then past months. */
export function selectableEndMonths(today: Date, count = 24): SelectableMonth[] {
	return selectableMonths(today, count);
}

// ------------------------------------------------------------------- deltas

export type DeltaKind = 'new' | 'gone' | 'up' | 'down' | 'flat' | 'none';

export interface Delta {
	/** `current - base`, signed. */
	abs: number;
	/**
	 * `abs / base` as a fraction. `null` when there is no base to divide by
	 * (a new category) and for a category that vanished: +/-100% would read as
	 * an ordinary change when it is really "this stopped".
	 */
	rel: number | null;
	kind: DeltaKind;
}

const EPSILON = 0.005;

export function delta(base: number, current: number): Delta {
	const baseZero = Math.abs(base) < EPSILON;
	const currentZero = Math.abs(current) < EPSILON;
	if (baseZero && currentZero) return { abs: 0, rel: null, kind: 'none' };
	if (baseZero) return { abs: current, rel: null, kind: 'new' };
	if (currentZero) return { abs: -base, rel: null, kind: 'gone' };
	const abs = current - base;
	if (Math.abs(abs) < EPSILON) return { abs: 0, rel: 0, kind: 'flat' };
	return { abs, rel: abs / base, kind: abs > 0 ? 'up' : 'down' };
}

/** "+12.5%", "-8%", "new", "gone", or an en dash when there is nothing to say. */
export function formatDeltaPercent(d: Delta): string {
	switch (d.kind) {
		case 'new':
			return 'new';
		case 'gone':
			return 'gone';
		case 'none':
			return '–';
		default: {
			const pct = (d.rel ?? 0) * 100;
			const text = Math.abs(pct).toLocaleString('en-US', { maximumFractionDigits: 1 });
			if (Math.abs(pct) < 0.05) return '0%';
			return `${pct > 0 ? '+' : '−'}${text}%`;
		}
	}
}

// -------------------------------------------------------------------- shape

export interface PeriodView {
	/** "Sep 2026". */
	shortLabel: string;
	uncategorized: number;
	label: string;
	start: string;
	end: string;
	total: number;
	/** The current month, cut off at today. */
	partial: boolean;
}

export interface RowView {
	slug: string;
	name: string;
	/** Amount per period, aligned with `ComparisonView.periods`. */
	cells: number[];
	/** Transactions per period, so a refunded-to-zero cell can still be drilled into. */
	counts: number[];
	base: number;
	current: number;
	delta: Delta;
}

export interface SideBucket {
	base: number;
	current: number;
	delta: Delta;
}

export interface ComparisonView {
	currency: string;
	periods: PeriodView[];
	baseIndex: number;
	currentIndex: number;
	total: SideBucket;
	/** Biggest movers first, by absolute change. */
	rows: RowView[];
	uncategorized: SideBucket;
	/** Held for review: shown beside the comparison, never inside it. */
	needsReview: SideBucket;
}

function periodLabel(p: { start: string }): string {
	const d = parseDateInputValue(p.start);
	return monthLabel(d.getFullYear(), d.getMonth() + 1);
}

/** Compact "Sep 2026" for chart axes. */
export function periodShortLabel(p: { start: string }): string {
	return parseDateInputValue(p.start).toLocaleDateString('en-US', {
		month: 'short',
		year: 'numeric'
	});
}

function clampIndex(i: number, length: number): number {
	return Math.min(Math.max(i, 0), Math.max(length - 1, 0));
}

/**
 * The two periods compared by default: the last two. When the last one is the
 * unfinished current month that would read as a collapse in spending, so the
 * last two complete months are compared instead (the partial month stays in
 * the chart and the table, and can be picked).
 */
export function defaultPair(
	periodCount: number,
	partialLast = false
): { base: number; current: number } {
	const last = partialLast && periodCount >= 3 ? periodCount - 2 : periodCount - 1;
	return { base: Math.max(last - 1, 0), current: Math.max(last, 0) };
}

/** Index of the period starting on `start` ("YYYY-MM-DD"), or `null`. */
export function periodIndexByStart(
	periods: Pick<ComparisonPeriod, 'start'>[],
	start: string | null | undefined
): number | null {
	if (!start) return null;
	const i = periods.findIndex((p) => p.start === start);
	return i === -1 ? null : i;
}

/**
 * Shape a `categoryComparison` response for the page. `baseIndex` and
 * `currentIndex` pick the two periods to diff; `partialLast` marks the final
 * period as the unfinished current month.
 */
export function shapeComparison(
	data: CategoryComparison,
	baseIndex: number,
	currentIndex: number,
	partialLast: boolean
): ComparisonView {
	const n = data.periods.length;
	const base = clampIndex(baseIndex, n);
	const current = clampIndex(currentIndex, n);

	const periods: PeriodView[] = data.periods.map((p, i) => ({
		label: periodLabel(p),
		shortLabel: periodShortLabel(p),
		uncategorized: decimalToNumber(p.uncategorized),
		start: p.start,
		end: p.end,
		total: decimalToNumber(p.total),
		partial: partialLast && i === n - 1
	}));

	const side = (get: (p: ComparisonPeriod) => string): SideBucket => {
		const b = n ? decimalToNumber(get(data.periods[base])) : 0;
		const c = n ? decimalToNumber(get(data.periods[current])) : 0;
		return { base: b, current: c, delta: delta(b, c) };
	};

	const rows: RowView[] = data.categories.map((c) => {
		const cells = c.cells.map((cell) => decimalToNumber(cell.amount));
		const b = cells[base] ?? 0;
		const cur = cells[current] ?? 0;
		return {
			slug: c.category.slug,
			name: c.category.name,
			cells,
			counts: c.cells.map((cell) => cell.transactionCount),
			base: b,
			current: cur,
			delta: delta(b, cur)
		};
	});
	rows.sort(
		(a, b) =>
			Math.abs(b.delta.abs) - Math.abs(a.delta.abs) ||
			b.current - a.current ||
			a.slug.localeCompare(b.slug)
	);

	const total = side((p) => p.total);
	return {
		currency: data.currency,
		periods,
		baseIndex: base,
		currentIndex: current,
		total,
		rows,
		uncategorized: side((p) => p.uncategorized),
		needsReview: side((p) => p.needsReview)
	};
}

// ----------------------------------------------------------------- drilldown

export interface DrillTarget {
	slug?: string;
	uncategorized?: boolean;
	needsReview?: boolean;
}

/**
 * Link to `/transactions` for one period and, optionally, one category. A
 * whole calendar month uses the `month:` preset so the period select shows it;
 * the unfinished current month uses `this-month`; anything else (a clipped
 * range) falls back to explicit dates.
 */
export function transactionsHref(
	period: { start: string; end: string },
	today: string,
	target: DrillTarget = {}
): string {
	const params = new URLSearchParams();
	const start = parseDateInputValue(period.start);
	const month = monthRange(start.getFullYear(), start.getMonth() + 1);
	if (period.start === month.start && period.end === month.end) {
		params.set('preset', monthSelection(start.getFullYear(), start.getMonth() + 1));
	} else if (period.start === month.start && period.end === today) {
		params.set('preset', 'this-month');
	} else {
		params.set('preset', 'custom');
		params.set('start', period.start);
		params.set('end', period.end);
	}
	if (target.slug) params.set('categorySlugs', target.slug);
	if (target.uncategorized) params.set('uncategorized', 'true');
	if (target.needsReview) params.set('needsReview', 'true');
	return `/transactions?${params.toString()}`;
}

// -------------------------------------------------------------------- chart

export interface TrendBar {
	label: string;
	start: string;
	/** The period's total, in exactly one of the three series below. */
	other: number;
	base: number;
	current: number;
}

/**
 * One bar per period. The total sits in `base`, `current` or `other` so the
 * chart can colour the two compared periods without a per-bar colour
 * accessor, and a stacked layout draws each bar once.
 */
export function trendBars(view: ComparisonView): TrendBar[] {
	return view.periods.map((p, i) => {
		const isCurrent = i === view.currentIndex;
		const isBase = i === view.baseIndex && !isCurrent;
		return {
			label: periodShortLabel(p) + (p.partial ? '*' : ''),
			start: p.start,
			other: isBase || isCurrent ? 0 : p.total,
			base: isBase ? p.total : 0,
			current: isCurrent ? p.total : 0
		};
	});
}
