/**
 * Mock data for `/admin/amazon`, served from `graphqlBackend.ts` in mock
 * mode so the page works with no backend.
 *
 * Kept out of `graphql/mocks/`, which is WP0-owned. It shows every state the
 * page has: a clean match, several `NEEDS_REVIEW` cases with the matcher's
 * real wording, an order with no bank line, an unclaimed bank line and a
 * Prime subscription charge. The amounts and ids are invented.
 *
 * Applying and undoing are exercisable too, and stateful for the life of the
 * server process (`resetAmazonMock()` clears it). The mock follows the real
 * rules, so the page can be reviewed against them: a single-item order has
 * nothing to split; one clean order's charge is marked as already split by the
 * user and is skipped and reported, never overwritten; undo removes only what
 * applying wrote.
 */

import type {
	AppliedPart,
	ChargeOutcome,
	PurchaseOrder,
	PurchaseOrderApplication,
	PurchaseOrderApplyResult,
	PurchaseOrderCharge,
	PurchaseOrderMatch,
	PurchaseOrderMatchReport,
	PurchaseOrderUploadResult
} from './types';

function order(
	orderId: string,
	orderDate: string,
	total: string,
	items: [string, string][],
	remainder = '0.0000'
): PurchaseOrder {
	return {
		orderId,
		orderDate,
		total,
		currency: 'EUR',
		remainder,
		items: items.map(([title, lineTotal], i) => ({
			asin: `B00MOCK${orderId.slice(-3)}${i}`,
			quantity: 1,
			title,
			lineTotal,
			status: 'Delivered',
			returned: false
		}))
	};
}

function charge(
	n: number,
	bookingDate: string,
	amount: string,
	description: string,
	counterpartyName = 'AMAZON PAYMENTS EUROPE S.C.A.'
): PurchaseOrderCharge {
	return {
		transactionId: `9c4e1b70-0000-4000-8000-00000000a${String(n).padStart(3, '0')}`,
		bookingDate,
		amount,
		currency: 'EUR',
		counterpartyName,
		description
	};
}

const CLEAN = order('303-9169011-8123555', '2026-10-08', '13.4500', [
	['Anker USB-C charger 100W', '13.4500']
]);
const SPLIT = order('304-9357930-8158714', '2026-09-23', '22.5900', [
	['Stainless steel cheese knife set', '22.5900']
]);
const SUM = order('305-1111111-2222222', '2026-09-12', '40.0000', [
	['Desk lamp', '25.0000'],
	['Cable ties, 200 pcs', '15.0000']
]);
const TWO = order('307-5555555-6666666', '2026-10-12', '20.5000', [
	['USB-C cable, 2 m', '8.5000'],
	['Wireless mouse', '12.0000']
]);
/** Its charge was split by hand, so applying must skip it. */
const BLOCKED = order('308-7777777-8888888', '2026-10-14', '40.0000', [
	['Office chair cushion', '30.0000'],
	['Desk mat', '10.0000']
]);
const GIFT = order('306-3333333-4444444', '2026-10-01', '5.0000', [['Greeting card', '5.0000']]);

const matches: PurchaseOrderMatch[] = [
	{
		order: CLEAN,
		kind: 'ORDER_ID',
		state: 'APPLY',
		reasons: [],
		charges: [charge(1, '2026-10-08', '-13.4500', '01303-9169011-8123555 AMZN Mktp DE')],
		refunds: [],
		applications: []
	},
	{
		order: SPLIT,
		kind: 'ORDER_ID',
		state: 'NEEDS_REVIEW',
		reasons: [
			{
				code: 'ITEMS_DO_NOT_RECONCILE_TO_CHARGES',
				message: 'order spans 2 charges; items do not reconcile to them'
			}
		],
		charges: [
			charge(2, '2026-09-23', '-8.9900', '01304-9357930-8158714 AMZN Mktp DE'),
			charge(3, '2026-09-24', '-13.6000', '01304-9357930-8158714 AMZN Mktp DE')
		],
		refunds: [],
		applications: []
	},
	{
		order: SUM,
		kind: 'ORDER_ID',
		state: 'NEEDS_REVIEW',
		reasons: [
			{
				code: 'CHARGES_DO_NOT_SUM_TO_TOTAL',
				message:
					'charges total 25.0000 but the order total is 40.0000; a shipment may be missing or still pending'
			}
		],
		charges: [charge(4, '2026-09-13', '-25.0000', '01305-1111111-2222222 AMZN Mktp DE')],
		refunds: [],
		applications: []
	},
	{
		order: TWO,
		kind: 'ORDER_ID',
		state: 'APPLY',
		reasons: [],
		charges: [charge(7, '2026-10-12', '-20.5000', '01307-5555555-6666666 AMZN Mktp DE')],
		refunds: [],
		applications: []
	},
	{
		order: BLOCKED,
		kind: 'ORDER_ID',
		state: 'APPLY',
		reasons: [],
		charges: [charge(8, '2026-10-14', '-40.0000', '01308-7777777-8888888 AMZN Mktp DE')],
		refunds: [],
		applications: []
	}
];

const report: PurchaseOrderMatchReport = {
	orderCount: 6,
	matches,
	unmatchedOrders: [GIFT],
	unmatchedCharges: [charge(5, '2026-10-02', '-7.7700', '01999-9999999-9999999 AMZN Mktp DE')],
	subscriptionCharges: [
		charge(6, '2026-10-07', '-8.9900', '01D01-8109513-9170236 AMZNPrime DE', 'AMAZON EU S.A R.L.')
	]
};

/** Orders applied so far in this mock session, by order id. */
const applied = new Map<string, PurchaseOrderApplication[]>();

export function resetAmazonMock(): void {
	applied.clear();
}

export function mockPurchaseOrderMatches(): { purchaseOrderMatches: PurchaseOrderMatchReport } {
	return {
		purchaseOrderMatches: {
			...report,
			matches: report.matches.map((m) => ({
				...m,
				applications: applied.get(m.order.orderId) ?? []
			}))
		}
	};
}

const BLOCKED_REASON = 'it already has a split that was not made from an order; left as it is';

function partsFor(m: PurchaseOrderMatch, categories: string[]): AppliedPart[] {
	return m.order.items.map((item, i) => ({
		amount: `-${item.lineTotal}`,
		categorySlug: categories[i] ?? '',
		label: item.title
	}));
}

type MockResult<T> = { data: T } | { errors: { message: string }[] };

/**
 * Mirrors `applyPurchaseOrderSplit`: refuses an order that is not a clean
 * match, skips a charge with fewer than two parts or one the user already
 * split, and is a no-op the second time.
 */
export function mockApplyPurchaseOrderSplit(
	variables: Record<string, unknown> | undefined
): MockResult<{ applyPurchaseOrderSplit: PurchaseOrderApplyResult }> {
	const input = variables?.input as
		| { orderId: string; itemCategories: string[]; remainderCategory: string | null }
		| undefined;
	const match = report.matches.find((m) => m.order.orderId === input?.orderId);
	if (!input || !match) return { errors: [{ message: 'no such order' }] };
	if (match.state !== 'APPLY') {
		return { errors: [{ message: 'this order needs review and cannot be applied' }] };
	}
	const outcomes: ChargeOutcome[] = match.charges.map((c): ChargeOutcome => {
		const base = { transactionId: c.transactionId, parts: [] as AppliedPart[] };
		if (match.order.items.length < 2) {
			return {
				...base,
				status: 'SKIPPED',
				reason:
					'there is nothing to split: the charge is a single item with no shipping or adjustments'
			};
		}
		if (match.order.orderId === BLOCKED.orderId) {
			return { ...base, status: 'SKIPPED', reason: BLOCKED_REASON };
		}
		const parts = partsFor(match, input.itemCategories);
		if (applied.has(match.order.orderId)) {
			return { ...base, status: 'ALREADY_APPLIED', reason: null, parts };
		}
		applied.set(match.order.orderId, [
			{ transactionId: c.transactionId, appliedAt: new Date().toISOString(), parts, intact: true }
		]);
		return { ...base, status: 'APPLIED', reason: null, parts };
	});
	return { data: { applyPurchaseOrderSplit: { orderId: match.order.orderId, outcomes } } };
}

/** Mirrors `undoPurchaseOrderSplit`: removes what applying wrote, nothing else. */
export function mockUndoPurchaseOrderSplit(variables: Record<string, unknown> | undefined): {
	undoPurchaseOrderSplit: PurchaseOrderApplyResult;
} {
	const orderId = String(variables?.orderId ?? '');
	const apps = applied.get(orderId) ?? [];
	applied.delete(orderId);
	return {
		undoPurchaseOrderSplit: {
			orderId,
			outcomes: apps.map((a) => ({
				transactionId: a.transactionId,
				status: 'UNDONE' as const,
				reason: null,
				parts: a.parts
			}))
		}
	};
}

/**
 * A stand-in for the upload: a file whose first column is not `Order ID` is
 * rejected whole with the real parser's wording; anything else "imports" its
 * distinct order ids. It reads no cell into the result.
 */
export function mockUploadPurchaseOrders(variables: Record<string, unknown> | undefined): {
	uploadPurchaseOrders: PurchaseOrderUploadResult;
} {
	const files = Array.isArray(variables?.files) ? (variables.files as string[]) : [];
	const result: PurchaseOrderUploadResult = {
		files: [],
		newOrders: 0,
		updatedOrders: 0,
		unchangedOrders: 0
	};
	files.forEach((text, index) => {
		const [header, ...rows] = text.split(/\r?\n/);
		if (!header.replace(/^﻿/, '').toLowerCase().startsWith('order id')) {
			result.files.push({
				index,
				orderCount: 0,
				error: 'unrecognised Amazon CSV header: column 1 should be "Order ID" but is something else'
			});
			return;
		}
		const ids = new Set(rows.map((r) => r.split(',')[0]).filter(Boolean));
		result.files.push({ index, orderCount: ids.size, error: null });
		result.newOrders += ids.size;
	});
	return { uploadPurchaseOrders: result };
}
