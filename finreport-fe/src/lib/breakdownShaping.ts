/**
 * Pure shaping of `CategoryBreakdown` GraphQL responses into the bar list
 * `CategoryBreakdown.svelte` renders (§5/§6 WP5). No Svelte, no I/O — covered
 * by `breakdownShaping.test.ts`.
 */

import { decimalToNumber } from './chartShaping';
import { descendantSlugs } from './categoryTree';
import type {
	Category,
	CategoryBreakdown,
	CategoryBreakdownRow,
	TransactionFilter
} from './graphql/types';

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
	/**
	 * Set on the row a child level carries for the parent's own transactions
	 * (labelled with the parent itself, not a subcategory). It repeats the
	 * parent's slug, so it is shown but is not drillable or expandable.
	 */
	own?: true;
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

/** What a bar click narrows the transaction list by: the `sel*` drill-down fields. */
export interface BarDrilldown {
	categorySlugs?: string[];
	uncategorized?: boolean;
	needsReview?: boolean;
}

/**
 * The drill-down a bar stands for. Every kind is actionable. `uncategorized`
 * also sets `needsReview: false`: the backend's `uncategorized` filter counts
 * a held (needs-review) label with no category as uncategorised, but the
 * breakdown reports those separately, so without it the list would hold more
 * rows than the bar's count.
 */
export function drilldownForBar(bar: BreakdownBar): BarDrilldown {
	switch (bar.kind) {
		case 'uncategorized':
			return { uncategorized: true, needsReview: false };
		case 'needs-review':
			return { needsReview: true };
		default:
			return bar.slug ? { categorySlugs: [bar.slug] } : {};
	}
}

/** Whether a category has (non-archived) subcategories to expand into. */
export function hasChildCategories(slug: string, categories: Category[]): boolean {
	const self = categories.find((c) => c.slug === slug);
	if (!self) return false;
	return categories.some((c) => c.parentId === self.id && !c.archived);
}

/** The breakdown `level` that shows a category's children (`depth + 1`; top level is 1). */
export function childLevel(slug: string, categories: Category[]): number {
	const depth = categories.find((c) => c.slug === slug)?.depth ?? 1;
	return depth + 1;
}

/**
 * The filter for a category's child breakdown: the period's own filter scoped
 * to the category (the backend includes descendants). When the filter panel
 * already restricts categories, the scope is the overlap, so expanding a row
 * never reveals a subcategory the panel has excluded.
 */
export function childBreakdownFilter(
	base: Partial<TransactionFilter>,
	slug: string,
	categories: Category[]
): Partial<TransactionFilter> {
	const panelSlugs = base.categorySlugs;
	if (!panelSlugs?.length) return { ...base, categorySlugs: [slug] };
	const inside = new Set(descendantSlugs(slug, categories));
	return { ...base, categorySlugs: panelSlugs.filter((s) => inside.has(s)) };
}

/**
 * Shape a child-level breakdown into bars. Only category rows are kept (the
 * scoped query has no uncategorized/needs-review rows to speak of). The row
 * that carries the parent's own slug holds transactions labelled with the
 * parent directly; it is kept so the children add up, marked `own`.
 */
export function shapeChildBreakdown(
	breakdown: CategoryBreakdown,
	parentSlug: string
): BreakdownBar[] {
	return breakdown.rows
		.map((row) => {
			const bar = toBar(row, 'category');
			return row.category.slug === parentSlug
				? { ...bar, label: `${bar.label} (no subcategory)`, own: true as const }
				: bar;
		})
		.sort((a, b) => Number(a.own === true) - Number(b.own === true) || b.amount - a.amount);
}
