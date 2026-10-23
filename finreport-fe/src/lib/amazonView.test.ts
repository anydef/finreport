import { describe, expect, it } from 'vitest';
import {
	mockApplyPurchaseOrderSplit,
	mockProposePurchaseOrderCategories,
	mockPurchaseOrderMatches,
	mockUndoPurchaseOrderSplit,
	mockUploadPurchaseOrders,
	resetAmazonMock
} from './graphql/amazonMock';
import type {
	PurchaseOrderItemProposal,
	PurchaseOrderMatch,
	PurchaseOrderUploadResult
} from './graphql/types';
import {
	applyBlocker,
	applyWarning,
	buildApplyInput,
	buildReview,
	categoryPickerGroups,
	chargedTotal,
	chargeLabel,
	describeOutcomes,
	emptyChoices,
	matchRow,
	reviewHeadline,
	uploadOutcome,
	applyProposals,
	proposalReasonText,
	proposalSourceLabel
} from './amazonView';
import type { Category } from './graphql/types';

const report = () => mockPurchaseOrderMatches().purchaseOrderMatches;

describe('buildReview', () => {
	it('puts clean matches, review cases, no-charge orders and subscriptions in their own sections', () => {
		const review = buildReview(report());
		expect(review.ready.map((r) => r.orderId)).toEqual([
			'303-9169011-8123555',
			'307-5555555-6666666',
			'308-7777777-8888888'
		]);
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
			'3 orders match cleanly, 2 need review, 1 with no bank charge.'
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

	it('says nothing was split whenever an order was read', () => {
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

	it('leaves off the split reassurance when no order was read', () => {
		const none: PurchaseOrderUploadResult = {
			files: [{ index: 0, orderCount: 0, error: 'bad header' }],
			newOrders: 0,
			updatedOrders: 0,
			unchangedOrders: 0
		};
		const out = uploadOutcome(none);
		expect(out.lines).toEqual(['File 1 was not imported: bad header']);
	});

	it('never returns an empty message', () => {
		const empty: PurchaseOrderUploadResult = {
			files: [],
			newOrders: 0,
			updatedOrders: 0,
			unchangedOrders: 0
		};
		expect(uploadOutcome(empty).lines).toEqual(['Nothing was imported.']);
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

describe('applying a match', () => {
	const rowFor = (orderId: string) =>
		matchRow(report().matches.find((m) => m.order.orderId === orderId)!);
	const two = () => rowFor('307-5555555-6666666');

	it('is blocked until every item has a category', () => {
		const row = two();
		const choices = emptyChoices(row.order);
		expect(applyBlocker(row, choices)).toBe('Choose a category for every item.');
		choices.itemCategories = ['a', ''];
		expect(applyBlocker(row, choices)).not.toBeNull();
		choices.itemCategories = ['a', 'b'];
		expect(applyBlocker(row, choices)).toBeNull();
	});

	it('is blocked for a single-item order, whatever is chosen', () => {
		const row = rowFor('303-9169011-8123555');
		const blocker = applyBlocker(row, { itemCategories: ['a'], remainderCategory: '' });
		expect(blocker).toContain('Nothing to split');
	});

	it('is blocked for an order the matcher sent to review', () => {
		const row = rowFor('305-1111111-2222222');
		expect(applyBlocker(row, { itemCategories: ['a', 'b'], remainderCategory: '' })).toContain(
			'no longer reports'
		);
	});

	it('sends only slugs: no amount ever comes from the client', () => {
		const row = two();
		const input = buildApplyInput(row, { itemCategories: ['a', 'b'], remainderCategory: '' });
		expect(input).toEqual({
			orderId: '307-5555555-6666666',
			itemCategories: ['a', 'b'],
			remainderCategory: null
		});
	});

	it('demands a category for a non-zero remainder and sends it', () => {
		const row = two();
		row.order = { ...row.order, remainder: '1.5000' };
		const choices = { itemCategories: ['a', 'b'], remainderCategory: '' };
		expect(applyBlocker(row, choices)).toContain('shipping');
		choices.remainderCategory = 'fees';
		expect(applyBlocker(row, choices)).toBeNull();
		expect(buildApplyInput(row, choices).remainderCategory).toBe('fees');
	});

	it('says plainly that it changes transactions and can be undone', () => {
		const warning = applyWarning(two());
		expect(warning).toContain('This changes your transactions');
		expect(warning).toContain('undo');
		expect(warning).toContain('already split or categorised yourself');
	});

	it('groups only live spending categories, keeping the parent path in labels', () => {
		const cat = (
			slug: string,
			kind: Category['kind'],
			parentId: string | null = null,
			archived = false
		): Category => ({
			id: slug,
			slug,
			name: slug,
			kind,
			parentId,
			depth: parentId ? 1 : 0,
			archived,
			origin: 'seed'
		});
		const groups = categoryPickerGroups([
			cat('food.groceries', 'EXPENSE', 'food'),
			cat('food', 'EXPENSE'),
			cat('income', 'INCOME'),
			cat('old', 'EXPENSE', null, true),
			cat('savings', 'SAVING')
		]);
		expect(groups.map((g) => g.groupLabel)).toEqual(['food', 'savings']);
		expect(groups[0].options.map((o) => o.slug)).toEqual(['food', 'food.groceries']);
		expect(groups[0].options[1].label).toBe('food / food.groceries');
	});
});

describe('the apply and undo mock', () => {
	const apply = (orderId: string) =>
		mockApplyPurchaseOrderSplit({
			input: { orderId, itemCategories: ['a', 'b'], remainderCategory: null }
		});

	it('applies, shows the order as applied, and undoes it back to clean', () => {
		resetAmazonMock();
		const out = apply('307-5555555-6666666');
		expect('data' in out && out.data.applyPurchaseOrderSplit.outcomes[0].status).toBe('APPLIED');

		const review = buildReview(report());
		expect(review.applied.map((r) => r.orderId)).toEqual(['307-5555555-6666666']);
		expect(review.ready.map((r) => r.orderId)).not.toContain('307-5555555-6666666');
		expect(reviewHeadline(review)).toContain('1 applied');

		const undone = mockUndoPurchaseOrderSplit({ orderId: '307-5555555-6666666' });
		expect(undone.undoPurchaseOrderSplit.outcomes[0].status).toBe('UNDONE');
		const after = buildReview(report());
		expect(after.applied).toHaveLength(0);
		expect(after.ready.map((r) => r.orderId)).toContain('307-5555555-6666666');
	});

	it('applying twice is a no-op', () => {
		resetAmazonMock();
		apply('307-5555555-6666666');
		const again = apply('307-5555555-6666666');
		expect('data' in again && again.data.applyPurchaseOrderSplit.outcomes[0].status).toBe(
			'ALREADY_APPLIED'
		);
		expect(buildReview(report()).applied[0].applications).toHaveLength(1);
	});

	it('skips a charge the user already split, and reports why', () => {
		resetAmazonMock();
		const out = apply('308-7777777-8888888');
		if (!('data' in out)) throw new Error('expected data');
		const outcome = out.data.applyPurchaseOrderSplit.outcomes[0];
		expect(outcome.status).toBe('SKIPPED');
		expect(outcome.reason).toContain('left as it is');
		expect(buildReview(report()).applied).toHaveLength(0);
	});

	it('refuses an order that needs review', () => {
		resetAmazonMock();
		expect('errors' in apply('305-1111111-2222222')).toBe(true);
	});
});

describe('describeOutcomes', () => {
	const part = { amount: '-1', categorySlug: 'a', label: 'x' };
	const outcome = (status: string, reason: string | null = null) =>
		({ transactionId: 't', status, reason, parts: [part, part] }) as never;

	it('counts what was split and names what was left alone', () => {
		const out = describeOutcomes(
			{
				orderId: 'o',
				outcomes: [outcome('APPLIED'), outcome('SKIPPED', 'it already has a split')]
			},
			'apply'
		);
		expect(out.tone).toBe('warn');
		expect(out.lines[0]).toBe('Split 1 transaction into 2 parts.');
		expect(out.lines).toContain('Left unchanged: it already has a split');
	});

	it('is a failure when nothing was changed', () => {
		const out = describeOutcomes({ orderId: 'o', outcomes: [outcome('SKIPPED', 'why')] }, 'apply');
		expect(out.tone).toBe('bad');
		expect(out.lines).toContain('No transaction was changed.');
	});

	it('reports a repeat as nothing changed', () => {
		const out = describeOutcomes({ orderId: 'o', outcomes: [outcome('ALREADY_APPLIED')] }, 'apply');
		expect(out.tone).toBe('good');
		expect(out.lines[0]).toContain('already split; nothing changed');
	});

	it('says a changed split was left alone on undo', () => {
		const out = describeOutcomes(
			{ orderId: 'o', outcomes: [outcome('LEFT_ALONE', 'changed since')] },
			'undo'
		);
		expect(out.tone).toBe('bad');
		expect(out.lines[0]).toBe('Left unchanged: changed since');
	});

	it('handles an undo with nothing to undo', () => {
		expect(describeOutcomes({ orderId: 'o', outcomes: [] }, 'undo').lines[0]).toBe(
			'Nothing to undo for this order.'
		);
	});
});

describe('proposals', () => {
	const known = new Set(['personal', 'household_items_supplies']);
	const p = (
		position: number,
		categorySlug: string | null,
		source: PurchaseOrderItemProposal['source'],
		reason: string | null = null
	): PurchaseOrderItemProposal => ({
		position,
		asin: `A${position}`,
		title: `t${position}`,
		categorySlug,
		source,
		confidence: null,
		reason
	});

	it('fills only empty pickers and keeps a slug the user chose', () => {
		const r = applyProposals(
			['', 'personal', ''],
			[p(0, 'personal', 'RULE'), p(1, 'household_items_supplies', 'LLM'), p(2, 'personal', 'LLM')],
			known
		);
		expect(r.itemCategories).toEqual(['personal', 'personal', 'personal']);
		expect(Object.keys(r.filled)).toEqual(['0', '2']);
		expect(r.notFilled[1].categorySlug).toBe('household_items_supplies');
	});

	it('leaves a NONE item empty and records why', () => {
		const r = applyProposals([''], [p(0, null, 'NONE', 'no_category')], known);
		expect(r.itemCategories).toEqual(['']);
		expect(r.notFilled[0].reason).toBe('no_category');
	});

	it('ignores an unknown slug and an out-of-range position', () => {
		const r = applyProposals([''], [p(0, 'gone', 'LLM'), p(5, 'personal', 'LLM')], known);
		expect(r.itemCategories).toEqual(['']);
		expect(r.filled).toEqual({});
	});

	it('words each source and tells a guess from a rule', () => {
		expect(proposalSourceLabel('RULE').text).toMatch(/rule/);
		expect(proposalSourceLabel('LLM').variant).toBe('warning');
		expect(proposalSourceLabel('NONE').text).toBe('No suggestion');
	});

	it('words reason codes, with the budget as a limit not a failure', () => {
		expect(proposalReasonText('llm_budget_exhausted')).toMatch(/limit/i);
		expect(proposalReasonText('llm_budget_exhausted')).not.toMatch(/fail/i);
		expect(proposalReasonText('provider_error')).toMatch(/failed/);
		expect(proposalReasonText('provider_unavailable')).toMatch(/configured/);
		expect(proposalReasonText('no_category')).toMatch(/matched/);
		expect(proposalReasonText('weird')).toBe('No suggestion available.');
		expect(proposalReasonText(null)).toBe('No suggestion available.');
	});
});

describe('mockProposePurchaseOrderCategories', () => {
	it('covers rule, AI and no-suggestion paths and suggests nothing for an unknown order', () => {
		const sources = ['307-5555555-6666666', '308-7777777-8888888'].flatMap((orderId) => {
			const r = mockProposePurchaseOrderCategories({ orderId });
			return 'data' in r ? r.data.proposePurchaseOrderCategories : [];
		});
		expect(new Set(sources.map((s) => s.source))).toEqual(new Set(['RULE', 'LLM', 'NONE']));
		expect(sources.find((s) => s.source === 'NONE')?.categorySlug).toBeNull();
		expect('errors' in mockProposePurchaseOrderCategories({ orderId: 'x' })).toBe(true);
	});
});
