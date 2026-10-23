/**
 * Pure shaping for `/admin/amazon`. No Svelte, no I/O - covered by
 * `amazonView.test.ts`.
 *
 * The matcher only calls a match `APPLY` when exactly one assignment of items
 * to charges reconciles; everything else is `NEEDS_REVIEW` with reasons the
 * backend already words. This module decides which section a row lands in and
 * how the sections are grouped and titled. It never rewords a reason: the
 * matcher's `message` is shown as is, and only the *group* titles and hints
 * are written here.
 */

import { decimalToNumber } from './chartShaping';
import type {
	PurchaseOrder,
	PurchaseOrderCharge,
	PurchaseOrderMatch,
	PurchaseOrderMatchReport,
	PurchaseOrderUploadResult
} from './graphql/types';

export interface MatchRow {
	orderId: string;
	orderDate: string;
	total: string;
	currency: string;
	itemCount: number;
	charges: PurchaseOrderCharge[];
	/** Sum of the charge magnitudes, rounded to cents. */
	chargedTotal: number;
	/** The matcher's own wording, one entry per reason. */
	reasons: string[];
	state: 'apply' | 'review';
	/** True when the join was on amount and date only, not the order id. */
	fuzzy: boolean;
	order: PurchaseOrder;
}

export interface ReviewGroup {
	/** The first reason's code, or `OTHER`. */
	code: string;
	title: string;
	/** What the group means and what to do about it. Written here, not by the matcher. */
	hint: string;
	rows: MatchRow[];
}

export interface AmazonReview {
	/** Orders that would apply cleanly. */
	ready: MatchRow[];
	/** Everything the matcher would not apply, grouped by why. */
	reviewGroups: ReviewGroup[];
	reviewCount: number;
	/** Orders with no bank line at all. */
	noCharge: PurchaseOrder[];
	/** Amazon-looking bank lines no order claims. */
	unclaimed: PurchaseOrderCharge[];
	/** Prime / Music charges, set aside: not orders. */
	subscriptions: PurchaseOrderCharge[];
	orderCount: number;
	/** No orders uploaded yet. */
	isEmpty: boolean;
}

/** Group order is the order worked through: the likeliest-to-resolve first. */
const GROUPS: { code: string; title: string; hint: string }[] = [
	{
		code: 'CHARGES_DO_NOT_SUM_TO_TOTAL',
		title: "Charges don't add up to the order",
		hint: 'A shipment may be missing from the imported bank history or still pending. Re-check after the next bank import.'
	},
	{
		code: 'ITEMS_DO_NOT_RECONCILE_TO_CHARGES',
		title: "Items don't fit the charges",
		hint: 'The order was charged in several parts and no way of grouping its items reproduces them. Nothing is guessed.'
	},
	{
		code: 'AMBIGUOUS_ITEM_ASSIGNMENT',
		title: 'More than one way to split the items',
		hint: 'Several groupings of the items fit the charges equally; only you can say which shipped when.'
	},
	{
		code: 'TOO_MANY_ITEMS',
		title: 'Too many items to assign',
		hint: 'Trying every grouping of this many items across charges is not attempted.'
	},
	{
		code: 'ORDER_ITEMS_DO_NOT_RECONCILE',
		title: "Items don't add up to the order total",
		hint: 'Shipping, gift cards or promotions the export does not explain. The difference would be its own part.'
	},
	{
		code: 'AMBIGUOUS_FALLBACK',
		title: 'Several lines could be this order',
		hint: 'The bank line has no order id, and more than one order or charge fits its amount and date.'
	},
	{
		code: 'AMOUNT_DATE_FALLBACK',
		title: 'Matched on amount and date only',
		hint: 'The bank line has no order id. A coincidence of amount and date is a guess, so it is never applied unconfirmed.'
	}
];

const OTHER_GROUP = {
	code: 'OTHER',
	title: 'Other',
	hint: 'The matcher flagged these for a reason this page does not know yet.'
};

export function chargedTotal(charges: PurchaseOrderCharge[]): number {
	const cents = charges.reduce(
		(sum, c) => sum + Math.round(Math.abs(decimalToNumber(c.amount)) * 100),
		0
	);
	return cents / 100;
}

export function matchRow(match: PurchaseOrderMatch): MatchRow {
	return {
		orderId: match.order.orderId,
		orderDate: match.order.orderDate,
		total: match.order.total,
		currency: match.order.currency,
		itemCount: match.order.items.length,
		charges: match.charges,
		chargedTotal: chargedTotal(match.charges),
		reasons: match.reasons.map((r) => r.message),
		state: match.state === 'APPLY' ? 'apply' : 'review',
		fuzzy: match.kind === 'AMOUNT_AND_DATE',
		order: match.order
	};
}

function groupFor(match: PurchaseOrderMatch): (typeof GROUPS)[number] {
	const code = match.reasons[0]?.code;
	return GROUPS.find((g) => g.code === code) ?? OTHER_GROUP;
}

export function buildReview(report: PurchaseOrderMatchReport): AmazonReview {
	const ready: MatchRow[] = [];
	const byCode = new Map<string, ReviewGroup>();
	for (const match of report.matches) {
		const row = matchRow(match);
		if (row.state === 'apply') {
			ready.push(row);
			continue;
		}
		const g = groupFor(match);
		let group = byCode.get(g.code);
		if (!group) {
			group = { code: g.code, title: g.title, hint: g.hint, rows: [] };
			byCode.set(g.code, group);
		}
		group.rows.push(row);
	}
	const known = GROUPS.map((g) => byCode.get(g.code)).filter((g): g is ReviewGroup => !!g);
	const other = byCode.get(OTHER_GROUP.code);
	const reviewGroups = other ? [...known, other] : known;
	return {
		ready,
		reviewGroups,
		reviewCount: reviewGroups.reduce((n, g) => n + g.rows.length, 0),
		noCharge: report.unmatchedOrders,
		unclaimed: report.unmatchedCharges,
		subscriptions: report.subscriptionCharges,
		orderCount: report.orderCount,
		isEmpty: report.orderCount === 0
	};
}

/** One line for the top of the page: where the user stands. */
export function reviewHeadline(review: AmazonReview): string {
	if (review.isEmpty) return 'No Amazon orders uploaded yet.';
	const parts = [
		`${review.ready.length} ${review.ready.length === 1 ? 'order' : 'orders'} match cleanly`,
		`${review.reviewCount} need${review.reviewCount === 1 ? 's' : ''} review`
	];
	if (review.noCharge.length > 0) parts.push(`${review.noCharge.length} with no bank charge`);
	return parts.join(', ') + '.';
}

export interface UploadOutcome {
	tone: 'good' | 'warn' | 'bad';
	/** Always says nothing was split, so an upload never reads as an action on transactions. */
	lines: string[];
}

function plural(n: number, one: string, many: string): string {
	return `${n} ${n === 1 ? one : many}`;
}

/** What to tell the user after an upload; rejected files are named, not hidden. */
export function uploadOutcome(result: PurchaseOrderUploadResult): UploadOutcome {
	const rejected = result.files.filter((f) => f.error);
	const lines: string[] = [];
	const read = result.newOrders + result.updatedOrders + result.unchangedOrders;
	if (read > 0) {
		const bits = [`${plural(result.newOrders, 'new order', 'new orders')} recorded`];
		if (result.updatedOrders > 0) bits.push(`${result.updatedOrders} updated`);
		if (result.unchangedOrders > 0) bits.push(`${result.unchangedOrders} already stored`);
		lines.push(bits.join(', ') + '.');
	}
	for (const f of rejected) {
		lines.push(`File ${f.index + 1} was not imported: ${f.error}`);
	}
	lines.push('No transaction was changed or split.');
	const tone = rejected.length === 0 ? 'good' : read > 0 ? 'warn' : 'bad';
	return { tone, lines };
}

/** Short label for a bank line in a list. */
export function chargeLabel(charge: PurchaseOrderCharge): string {
	return charge.counterpartyName ?? charge.description ?? 'Bank line';
}
