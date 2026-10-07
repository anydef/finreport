/**
 * Pure shaping of `CategoryBreakdown` GraphQL responses into the bar list
 * `CategoryBreakdown.svelte` renders (§5/§6 WP5). No Svelte, no I/O — covered
 * by `breakdownShaping.test.ts`.
 */

import { decimalToNumber } from './chartShaping';
import type { CategoryBreakdown, CategoryBreakdownRow } from './graphql/types';

export interface BreakdownBar {
	/** Stable React/Svelte `#each` key. */
	key: string;
	label: string;
	/** `null` for the synthetic uncategorized/needs-review rows (no slug to filter by). */
	slug: string | null;
	amount: number;
	/** `0..1`, already relative to the row's own `kind` total (§5). */
	share: number;
	transactionCount: number;
	/** Distinguishes the two synthetic rows from a real category row. */
	kind: 'category' | 'uncategorized' | 'needs-review';
}

function toBar(row: CategoryBreakdownRow, kind: BreakdownBar['kind']): BreakdownBar {
	return {
		key: kind === 'category' ? row.category.slug : kind,
		label: row.category.name,
		slug: kind === 'category' ? row.category.slug : null,
		amount: decimalToNumber(row.amount),
		share: row.share,
		transactionCount: row.transactionCount,
		kind
	};
}

/**
 * Shape a `CategoryBreakdown` into bars sorted by amount (largest first),
 * with `uncategorized`/`needsReview` appended last when present — they are
 * never part of the ranked "top categories", just always-visible totals
 * (§5: needs-review is a decision pending, not an absence).
 */
export function shapeCategoryBreakdown(breakdown: CategoryBreakdown): BreakdownBar[] {
	const rows = breakdown.rows
		.map((row) => toBar(row, 'category'))
		.sort((a, b) => b.amount - a.amount);
	if (breakdown.uncategorized) rows.push(toBar(breakdown.uncategorized, 'uncategorized'));
	if (breakdown.needsReview) rows.push(toBar(breakdown.needsReview, 'needs-review'));
	return rows;
}

/** The largest bar's amount, used as the 100%-width reference for the bar chart (0 when empty). */
export function maxBarAmount(bars: BreakdownBar[]): number {
	return bars.reduce((max, bar) => Math.max(max, bar.amount), 0);
}
