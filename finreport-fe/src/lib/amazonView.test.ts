import { describe, expect, it } from 'vitest';
import { mockPurchaseOrderMatches, mockUploadPurchaseOrders } from './graphql/amazonMock';
import type { PurchaseOrderMatch, PurchaseOrderUploadResult } from './graphql/types';
import {
	buildReview,
	chargedTotal,
	chargeLabel,
	reviewHeadline,
	uploadOutcome
} from './amazonView';

const report = () => mockPurchaseOrderMatches().purchaseOrderMatches;

describe('buildReview', () => {
	it('puts clean matches, review cases, no-charge orders and subscriptions in their own sections', () => {
		const review = buildReview(report());
		expect(review.ready.map((r) => r.orderId)).toEqual(['303-9169011-8123555']);
		expect(review.reviewCount).toBe(2);
		expect(review.noCharge.map((o) => o.orderId)).toEqual(['306-3333333-4444444']);
		expect(review.unclaimed).toHaveLength(1);
		expect(review.subscriptions).toHaveLength(1);
		expect(review.isEmpty).toBe(false);
	});

	it('never mixes a subscription into the unclaimed lines', () => {
		const review = buildReview(report());
		const ids = new Set(review.unclaimed.map((c) => c.transactionId));
		expect(review.subscriptions.every((c) => !ids.has(c.transactionId))).toBe(true);
	});

	it('groups review cases by their first reason, in a fixed working order', () => {
		const review = buildReview(report());
		expect(review.reviewGroups.map((g) => g.code)).toEqual([
			'CHARGES_DO_NOT_SUM_TO_TOTAL',
			'ITEMS_DO_NOT_RECONCILE_TO_CHARGES'
		]);
	});

	it("shows the matcher's own wording on each row, unchanged", () => {
		const review = buildReview(report());
		const split = review.reviewGroups
			.flatMap((g) => g.rows)
			.find((r) => r.orderId === '304-9357930-8158714');
		expect(split?.reasons).toEqual(['order spans 2 charges; items do not reconcile to them']);
		expect(split?.state).toBe('review');
	});

	it('puts a reason it has never seen in an Other group, last', () => {
		const base = report();
		const odd: PurchaseOrderMatch = {
			...base.matches[1],
			reasons: [{ code: 'SOMETHING_NEW', message: 'a new kind of trouble' }]
		};
		const review = buildReview({ ...base, matches: [odd, ...base.matches] });
		const codes = review.reviewGroups.map((g) => g.code);
		expect(codes[codes.length - 1]).toBe('OTHER');
		expect(review.reviewGroups.at(-1)?.rows[0].reasons).toEqual(['a new kind of trouble']);
	});

	it('flags a join made on amount and date only', () => {
		const base = report();
		const fuzzy: PurchaseOrderMatch = {
			...base.matches[1],
			kind: 'AMOUNT_AND_DATE',
			reasons: [{ code: 'AMOUNT_DATE_FALLBACK', message: 'matched on amount and date only' }]
		};
		const row = buildReview({ ...base, matches: [fuzzy] }).reviewGroups[0].rows[0];
		expect(row.fuzzy).toBe(true);
	});

	it('is empty when nothing was uploaded', () => {
		const review = buildReview({
			orderCount: 0,
			matches: [],
			unmatchedOrders: [],
			unmatchedCharges: [],
			subscriptionCharges: []
		});
		expect(review.isEmpty).toBe(true);
		expect(reviewHeadline(review)).toBe('No Amazon orders uploaded yet.');
	});
});

describe('chargedTotal', () => {
	it('sums magnitudes to the cent', () => {
		const [a, b] = report().matches[1].charges;
		expect(chargedTotal([a, b])).toBe(22.59);
	});
});

describe('reviewHeadline', () => {
	it('counts the sections', () => {
		expect(reviewHeadline(buildReview(report()))).toBe(
			'1 order match cleanly, 2 need review, 1 with no bank charge.'
		);
	});
});

describe('chargeLabel', () => {
	it('falls back from counterparty to description', () => {
		const c = report().unmatchedCharges[0];
		expect(chargeLabel({ ...c, counterpartyName: null, description: 'x' })).toBe('x');
		expect(chargeLabel({ ...c, counterpartyName: null, description: null })).toBe('Bank line');
	});
});

describe('uploadOutcome', () => {
	const ok: PurchaseOrderUploadResult = {
		files: [{ index: 0, orderCount: 3, error: null }],
		newOrders: 2,
		updatedOrders: 1,
		unchangedOrders: 0
	};

	it('always says nothing was split', () => {
		expect(uploadOutcome(ok).lines.at(-1)).toBe('No transaction was changed or split.');
		expect(uploadOutcome(ok).tone).toBe('good');
	});

	it('names a rejected file and keeps the good ones', () => {
		const mixed: PurchaseOrderUploadResult = {
			...ok,
			files: [
				{ index: 0, orderCount: 0, error: 'line 3: column "Order Date": not a date' },
				{ index: 1, orderCount: 3, error: null }
			]
		};
		const out = uploadOutcome(mixed);
		expect(out.tone).toBe('warn');
		expect(out.lines.join('\n')).toContain('File 1 was not imported: line 3: column "Order Date"');
		expect(out.lines[0]).toContain('2 new orders recorded');
	});

	it('is an error when nothing at all was read', () => {
		const none: PurchaseOrderUploadResult = {
			files: [{ index: 0, orderCount: 0, error: 'bad header' }],
			newOrders: 0,
			updatedOrders: 0,
			unchangedOrders: 0
		};
		expect(uploadOutcome(none).tone).toBe('bad');
	});
});

describe('mockUploadPurchaseOrders', () => {
	it('rejects a file with the wrong header without echoing a row', () => {
		const { uploadPurchaseOrders } = mockUploadPurchaseOrders({
			files: ['Foo,Bar\nSecret Name,Secret Street\n']
		});
		expect(uploadPurchaseOrders.files[0].error).toContain('column 1');
		expect(uploadPurchaseOrders.files[0].error).not.toContain('Secret');
	});

	it('counts distinct orders in a file with the right header', () => {
		const { uploadPurchaseOrders } = mockUploadPurchaseOrders({
			files: ['Order ID,Order Date\nA,x\nA,x\nB,x\n']
		});
		expect(uploadPurchaseOrders.files[0]).toEqual({ index: 0, orderCount: 2, error: null });
	});
});
