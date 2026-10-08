/**
 * The dashboard's "needs attention" summary: which buckets to show, how to
 * word them and where each one links. Pure so the quiet/zero behaviour and the
 * link targets are unit-tested.
 *
 * The summary is **all-time** (the backend query takes no period): a backlog
 * outside the dashboard's period is still a backlog, so the figures never
 * move with the period selector and the links open an all-time list.
 */

import type { AttentionSummary } from './graphql/types';

export interface AttentionItem {
	key: 'uncategorized' | 'needsReview';
	count: number;
	label: string;
	/** Unsigned worth of the bucket, e.g. "4,120.50 EUR". */
	worth: string;
	href: string;
}

/** Start of the "everything" range the uncategorised link opens. */
export const ALL_TIME_START = '2000-01-01';

/** "1,234.50 EUR" — a magnitude, so no leading sign. */
export function formatWorth(value: string, currency: string): string {
	const n = Math.abs(Number(value));
	if (Number.isNaN(n)) return `${value} ${currency}`;
	return `${n.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 })} ${currency}`;
}

/** The transaction list, all time, narrowed to transactions with no category. */
export function uncategorizedHref(today: string): string {
	const params = new URLSearchParams({
		preset: 'custom',
		start: ALL_TIME_START,
		end: today,
		uncategorized: 'true'
	});
	return `/transactions?${params.toString()}`;
}

/** Only buckets with something in them, uncategorised first. Empty = nothing to do. */
export function attentionItems(
	summary: AttentionSummary,
	currency: string,
	today: string
): AttentionItem[] {
	const items: AttentionItem[] = [];
	if (summary.uncategorized.count > 0) {
		items.push({
			key: 'uncategorized',
			count: summary.uncategorized.count,
			label: 'uncategorised',
			worth: formatWorth(summary.uncategorized.totalAmount, currency),
			href: uncategorizedHref(today)
		});
	}
	if (summary.needsReview.count > 0) {
		items.push({
			key: 'needsReview',
			count: summary.needsReview.count,
			label: 'held for review',
			worth: formatWorth(summary.needsReview.totalAmount, currency),
			href: '/review'
		});
	}
	return items;
}
