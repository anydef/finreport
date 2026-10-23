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
import { categoryOptionGroups, type CategoryOptionGroup } from './categoryTree';
import type {
	AppliedAs,
	ApplyPurchaseOrderInput,
	Category,
	ChargeOutcome,
	ProposalSource,
	PurchaseOrder,
	PurchaseOrderItemProposal,
	PurchaseOrderApplication,
	PurchaseOrderApplyResult,
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
	/** Splits already made from this order; empty until the user applies it. */
	applications: PurchaseOrderApplication[];
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
	/** Orders that would apply cleanly and have not been applied. */
	ready: MatchRow[];
	/** Orders the user has applied (whatever the matcher says about them now). */
	applied: MatchRow[];
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
		order: match.order,
		applications: match.applications ?? []
	};
}

function groupFor(match: PurchaseOrderMatch): (typeof GROUPS)[number] {
	const code = match.reasons[0]?.code;
	return GROUPS.find((g) => g.code === code) ?? OTHER_GROUP;
}

export function buildReview(report: PurchaseOrderMatchReport): AmazonReview {
	const ready: MatchRow[] = [];
	const applied: MatchRow[] = [];
	const byCode = new Map<string, ReviewGroup>();
	for (const match of report.matches) {
		const row = matchRow(match);
		// Applied wins over the matcher's current opinion: the user must always
		// be able to see, and undo, what they applied.
		if (row.applications.length > 0) {
			applied.push(row);
			continue;
		}
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
		applied,
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
	if (review.applied.length > 0) parts.push(`${review.applied.length} applied`);
	if (review.noCharge.length > 0) parts.push(`${review.noCharge.length} with no bank charge`);
	return parts.join(', ') + '.';
}

export interface UploadOutcome {
	tone: 'good' | 'warn' | 'bad';
	/**
	 * Says nothing was split whenever an order was read, so an upload never
	 * reads as an action on transactions. A file rejected whole read nothing,
	 * so that reassurance is left off — there was no order to act on.
	 */
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
	if (read > 0) lines.push('No transaction was changed or split.');
	if (lines.length === 0) lines.push('Nothing was imported.');
	const tone = rejected.length === 0 ? 'good' : read > 0 ? 'warn' : 'bad';
	return { tone, lines };
}

/** Short label for a bank line in a list. */
export function chargeLabel(charge: PurchaseOrderCharge): string {
	return charge.counterpartyName ?? charge.description ?? 'Bank line';
}

// -- Applying a match -----------------------------------------------------------
//
// Applying is the one action on this page that changes transactions. These
// functions decide whether the button may be pressed and what is sent; the
// server re-verifies everything and computes every amount itself, so the only
// thing the client contributes is a category slug per item.

export interface ApplyChoices {
	/** One slug per item of the order, in item order; '' = not chosen. */
	itemCategories: string[];
	remainderCategory: string;
}

export function emptyChoices(order: PurchaseOrder): ApplyChoices {
	return { itemCategories: order.items.map(() => ''), remainderCategory: '' };
}

const isZero = (decimal: string): boolean => decimalToNumber(decimal) === 0;

/** Items that become a part: a free (zero) item makes none and needs no category. */
function splittableIndexes(order: PurchaseOrder): number[] {
	return order.items.flatMap((item, i) => (isZero(item.lineTotal) ? [] : [i]));
}

/** Why Apply is not available yet, or null when it is. */
export function applyBlocker(row: MatchRow, choices: ApplyChoices): string | null {
	if (row.state !== 'apply') return 'The matcher no longer reports this order as a clean match.';
	const needed = splittableIndexes(row.order);
	const hasRemainder = !isZero(row.order.remainder);
	if (needed.length + (hasRemainder ? 1 : 0) < 1) {
		return 'Nothing to apply: no item has an amount.';
	}
	if (needed.some((i) => !choices.itemCategories[i])) {
		return 'Choose a category for every item.';
	}
	if (hasRemainder && !choices.remainderCategory) {
		return 'Choose a category for shipping and adjustments.';
	}
	return null;
}

export function buildApplyInput(row: MatchRow, choices: ApplyChoices): ApplyPurchaseOrderInput {
	return {
		orderId: row.orderId,
		itemCategories: row.order.items.map((_, i) => choices.itemCategories[i] ?? ''),
		remainderCategory: isZero(row.order.remainder) ? null : choices.remainderCategory || null
	};
}

/**
 * True when the order has exactly one part to write (one priced item, no
 * shipping or adjustments). Such a charge has nothing to divide, so the server
 * sets its category instead of splitting it. A multi-charge order may still
 * have single-part charges; the outcome's `appliedAs` reports those.
 */
export function isSinglePart(row: MatchRow): boolean {
	const parts = splittableIndexes(row.order).length + (isZero(row.order.remainder) ? 0 : 1);
	return parts === 1;
}

/** The button that opens the confirmation. */
export function applyActionLabel(row: MatchRow): string {
	return isSinglePart(row) ? 'Set category…' : 'Apply split…';
}

/** The button that confirms it. */
export function confirmActionLabel(row: MatchRow): string {
	return isSinglePart(row) ? 'Yes, set the category' : 'Yes, split the transaction';
}

/** Intro for the "match cleanly" card. */
export function readyIntro(): string {
	return (
		'Exactly one way of assigning the items to the charges reconciles. Nothing has been applied. ' +
		'Applying changes your transactions: a bank charge with several items is split into one part per ' +
		'item, and one with a single item gets that item’s category, in the categories you choose. ' +
		'It never replaces a split or category you set yourself, and you can undo it.'
	);
}

/** Plain statement of what pressing Apply will do, shown before it is confirmed. */
export function applyWarning(row: MatchRow): string {
	const n = row.charges.length;
	const charges = n === 1 ? 'the bank charge' : `each of the ${n} bank charges`;
	if (isSinglePart(row)) {
		return (
			`This changes your transactions. ${charges[0].toUpperCase()}${charges.slice(1)} for this order ` +
			`is a single item, so it is not split: the category you chose will be set on the transaction. ` +
			`A transaction you have already split or categorised yourself is left as it is. You can undo it afterwards.`
		);
	}
	return (
		`This changes your transactions. ${charges[0].toUpperCase()}${charges.slice(1)} for this order ` +
		`will be replaced by one split part per item, in the categories you chose. A transaction you have ` +
		`already split or categorised yourself is left as it is. You can undo it afterwards.`
	);
}

/**
 * Spending categories to choose from, grouped for `SearchMenu`: live, expense or saving.
 * Paths are built from the full list so a kept child still reads `Parent / Child`.
 */
export function categoryPickerGroups(categories: Category[]): CategoryOptionGroup[] {
	const allowed = new Set(
		categories
			.filter((c) => !c.archived && (c.kind === 'EXPENSE' || c.kind === 'SAVING'))
			.map((c) => c.slug)
	);
	return categoryOptionGroups(categories)
		.map((g) => ({ ...g, options: g.options.filter((o) => allowed.has(o.slug)) }))
		.filter((g) => g.options.length > 0);
}

/** What an application on record did to its transaction. */
export function applicationLabel(
	app: Pick<PurchaseOrderApplication, 'appliedAs' | 'parts'>
): string {
	return app.appliedAs === 'CATEGORY'
		? 'Category set'
		: `Split into ${plural(app.parts.length, 'part', 'parts')}`;
}

/** Warning for an application the user changed since, or null while it is intact. */
export function intactNote(
	app: Pick<PurchaseOrderApplication, 'appliedAs' | 'intact'>
): string | null {
	if (app.intact) return null;
	return app.appliedAs === 'CATEGORY'
		? 'The category was changed since it was applied: undo will not touch it.'
		: 'Changed since it was applied: undo will not touch it.';
}

/** Intro for the "Applied" card, from what the applications actually were. */
export function appliedIntro(rows: MatchRow[]): string {
	const kinds = new Set<AppliedAs>(rows.flatMap((r) => r.applications.map((a) => a.appliedAs)));
	const wrote =
		kinds.size === 1 && kinds.has('CATEGORY')
			? 'set categories on'
			: kinds.size === 1
				? 'split'
				: 'split or set categories on';
	const removes =
		kinds.size === 1 && kinds.has('CATEGORY')
			? 'the category'
			: kinds.size === 1
				? 'the split'
				: 'the split or category';
	return (
		`These orders have ${wrote} your transactions. Undo removes ${removes} and the transaction ` +
		`goes back to its automatic category, as it was before. If you have changed it yourself since, ` +
		`undo leaves your version alone.`
	);
}

export interface ActionOutcome {
	tone: 'good' | 'warn' | 'bad';
	lines: string[];
}

function count(outcomes: ChargeOutcome[], status: ChargeOutcome['status']): ChargeOutcome[] {
	return outcomes.filter((o) => o.status === status);
}

/** What to tell the user after an apply or an undo. Skipped charges are named with their reason. */
export function describeOutcomes(
	result: PurchaseOrderApplyResult,
	mode: 'apply' | 'undo'
): ActionOutcome {
	const { outcomes } = result;
	const lines: string[] = [];
	if (outcomes.length === 0) {
		return {
			tone: 'warn',
			lines: [mode === 'undo' ? 'Nothing to undo for this order.' : 'No charge was found to apply.']
		};
	}
	const done = count(outcomes, mode === 'apply' ? 'APPLIED' : 'UNDONE');
	const already = count(outcomes, 'ALREADY_APPLIED');
	const problems = [...count(outcomes, 'SKIPPED'), ...count(outcomes, 'LEFT_ALONE')];
	// Anything not reported as a category is a split, as before `appliedAs` existed.
	const asCategory = (list: ChargeOutcome[]) => list.filter((o) => o.appliedAs === 'CATEGORY');
	const asSplit = (list: ChargeOutcome[]) => list.filter((o) => o.appliedAs !== 'CATEGORY');
	const split = asSplit(done);
	const category = asCategory(done);
	if (split.length > 0) {
		const verb = mode === 'apply' ? 'Split' : 'Restored';
		lines.push(
			`${verb} ${plural(split.length, 'transaction', 'transactions')}` +
				(mode === 'apply'
					? ` into ${plural(
							split.reduce((n, o) => n + o.parts.length, 0),
							'part',
							'parts'
						)}.`
					: ' to how they were before.')
		);
	}
	if (category.length > 0) {
		lines.push(
			mode === 'apply'
				? `Category set on ${plural(category.length, 'transaction', 'transactions')}.`
				: `Category removed from ${plural(category.length, 'transaction', 'transactions')}; back to how ${category.length === 1 ? 'it was' : 'they were'} before.`
		);
	}
	const alreadySplit = asSplit(already);
	const alreadyCategory = asCategory(already);
	if (alreadySplit.length > 0) {
		lines.push(
			`${plural(alreadySplit.length, 'transaction was', 'transactions were')} already split; nothing changed.`
		);
	}
	if (alreadyCategory.length > 0) {
		lines.push(
			`${plural(alreadyCategory.length, 'transaction', 'transactions')} already had the category set; nothing changed.`
		);
	}
	for (const o of problems) {
		lines.push(`Left unchanged: ${o.reason ?? 'no reason given'}`);
	}
	if (done.length === 0 && already.length === 0) lines.push('No transaction was changed.');
	const tone = problems.length === 0 ? 'good' : done.length + already.length > 0 ? 'warn' : 'bad';
	return { tone, lines };
}

// -- Category proposals: suggestions that prefill the pickers, never apply.

export type ProposalBadgeVariant = 'neutral' | 'info' | 'success' | 'warning';

/** Where a suggestion came from, worded so a guess does not read as a fact. */
export function proposalSourceLabel(source: ProposalSource): {
	text: string;
	variant: ProposalBadgeVariant;
} {
	switch (source) {
		case 'RULE':
			return { text: 'Suggested by a rule', variant: 'success' };
		case 'CACHE':
			return { text: 'Suggested from an earlier AI answer', variant: 'info' };
		case 'LLM':
			return { text: 'Suggested by AI, check it', variant: 'warning' };
		default:
			return { text: 'No suggestion', variant: 'neutral' };
	}
}

/** Short human text for the backend's machine-readable `NONE` reason code. */
export function proposalReasonText(reason: string | null): string {
	switch (reason) {
		case 'no_category':
			return 'Nothing matched this item.';
		case 'provider_error':
			return 'The suggestion service failed for this item. Try again later.';
		case 'provider_unavailable':
			return 'No suggestion service is configured.';
		case 'llm_budget_exhausted':
			return 'AI suggestion limit reached for now; choose a category yourself.';
		default:
			return 'No suggestion available.';
	}
}

export interface ProposalApplication {
	/** The choices with empty pickers filled; a picker already holding a slug is untouched. */
	itemCategories: string[];
	/** Item index -> the proposal that filled it (only those it actually filled). */
	filled: Record<number, ItemProposalView>;
	/** Item index -> proposal for items left as they were: kept choice or no suggestion. */
	notFilled: Record<number, ItemProposalView>;
}

export type ItemProposalView = Pick<
	PurchaseOrderItemProposal,
	'categorySlug' | 'source' | 'reason' | 'confidence'
>;

/**
 * Merges proposals into the current choices. A choice is "made" when its slug
 * is non-empty, so only empty pickers are filled; a slug the proposal names
 * that is not a known category is ignored (stays empty).
 */
export function applyProposals(
	current: string[],
	proposals: PurchaseOrderItemProposal[],
	knownSlugs: ReadonlySet<string>
): ProposalApplication {
	const itemCategories = [...current];
	const filled: Record<number, ItemProposalView> = {};
	const notFilled: Record<number, ItemProposalView> = {};
	for (const p of proposals) {
		if (p.position < 0 || p.position >= current.length) continue;
		const view = {
			categorySlug: p.categorySlug,
			source: p.source,
			reason: p.reason,
			confidence: p.confidence
		};
		const slug = p.categorySlug;
		if (!current[p.position] && slug && p.source !== 'NONE' && knownSlugs.has(slug)) {
			itemCategories[p.position] = slug;
			filled[p.position] = view;
		} else {
			notFilled[p.position] = view;
		}
	}
	return { itemCategories, filled, notFilled };
}
