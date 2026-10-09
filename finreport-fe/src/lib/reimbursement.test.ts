import { describe, expect, it } from 'vitest';
import {
	candidateScore,
	dialogMemberFromLink,
	dialogMemberFromTransaction,
	linkInput,
	roleForAmount,
	saveBlocker,
	formatMagnitude,
	linkHint,
	linkabilityOfSelection,
	ownRole,
	previewOffsets,
	reimbursementChoiceFromParam,
	reimbursementFilterValue,
	summaryLine
} from './reimbursement';
import type { TransactionLink, TransactionLinkMember } from './graphql/types';

function member(over: Partial<TransactionLinkMember>): TransactionLinkMember {
	return {
		transactionId: 'x',
		role: 'EXPENSE',
		bookingDate: '2024-02-14',
		amount: '-1000.00',
		currency: 'EUR',
		counterpartyName: 'Praxis Dr. Meyer',
		description: null,
		allocated: '600.00',
		remaining: '400.00',
		...over
	};
}

function link(over: Partial<TransactionLink> & { members: TransactionLinkMember[] }): TransactionLink {
	return {
		id: 'l1',
		kind: 'REIMBURSEMENT',
		note: null,
		status: 'PARTIAL',
		currency: 'EUR',
		expenseTotal: '1000.00',
		offsetTotal: '600.00',
		reimbursed: '600.00',
		net: '400.00',
		surplus: '0.00',
		missingMembers: 0,
		...over
	};
}

const partial = link({
	members: [
		member({ transactionId: 'bill' }),
		member({
			transactionId: 'back',
			role: 'OFFSET',
			amount: '600.00',
			counterpartyName: 'Debeka',
			bookingDate: '2024-03-06',
			allocated: '600.00',
			remaining: '0.00'
		})
	]
});

describe('previewOffsets', () => {
	it('a full reimbursement nets to zero', () => {
		expect(previewOffsets(['-1000'], ['1000'])).toMatchObject({
			status: 'FULL',
			reimbursed: '1000.00',
			net: '0.00',
			surplus: '0.00'
		});
	});

	it('a partial one leaves the rest as the net cost', () => {
		expect(previewOffsets(['-1000.00'], ['600.00'])).toMatchObject({
			status: 'PARTIAL',
			reimbursed: '600.00',
			net: '400.00'
		});
	});

	it('more back than paid caps the offset and reports the surplus, never a negative net', () => {
		expect(previewOffsets(['-50'], ['80'])).toMatchObject({
			status: 'OVER',
			reimbursed: '50.00',
			net: '0.00',
			surplus: '30.00'
		});
	});

	it('sums several on either side', () => {
		expect(previewOffsets(['-100', '-300'], ['200'])).toMatchObject({
			expenseTotal: '400.00',
			offsetTotal: '200.00',
			net: '200.00'
		});
		expect(previewOffsets(['-900'], ['300', '300']).reimbursed).toBe('600.00');
	});

	it('is exact in cents (no float drift)', () => {
		expect(previewOffsets(['-0.10', '-0.20'], ['0.30']).status).toBe('FULL');
	});

	it('a side with nothing on it is incomplete and offsets nothing', () => {
		expect(previewOffsets(['-10'], [])).toMatchObject({ status: 'INCOMPLETE', reimbursed: '0.00' });
	});
});

describe('linkHint', () => {
	it('an expense states what was paid, reimbursed and what it still costs', () => {
		const hint = linkHint({ id: 'bill', currency: 'EUR', link: partial })!;
		expect(hint.role).toBe('EXPENSE');
		expect(hint.text).toBe(
			'1,000.00 EUR paid, 600.00 EUR reimbursed, 400.00 EUR net (partly reimbursed)'
		);
	});

	it('a fully offset expense says so', () => {
		const full = link({
			status: 'FULL',
			members: [member({ transactionId: 'bill', allocated: '1000.00', remaining: '0.00' })]
		});
		expect(linkHint({ id: 'bill', currency: 'EUR', link: full })!.text).toContain(
			'(fully reimbursed)'
		);
	});

	it('a reimbursement names the expense it offsets', () => {
		const hint = linkHint({ id: 'back', currency: 'EUR', link: partial })!;
		expect(hint.role).toBe('OFFSET');
		expect(hint.text).toContain('Offsets Praxis Dr. Meyer, 14 Feb 2024');
		expect(hint.text).toContain('expense only partly covered');
	});

	it('names how many more expenses a many-to-one reimbursement spans', () => {
		const many = link({
			members: [
				member({ transactionId: 'a' }),
				member({ transactionId: 'b', counterpartyName: 'Sixt' }),
				member({
					transactionId: 'back',
					role: 'OFFSET',
					amount: '200.00',
					allocated: '200.00',
					remaining: '0.00'
				})
			]
		});
		expect(linkHint({ id: 'back', currency: 'EUR', link: many })!.text).toContain('and 1 more');
	});

	it('flags a surplus on the reimbursement side', () => {
		const over = link({
			status: 'OVER',
			members: [
				member({ transactionId: 'bill', amount: '-50.00', allocated: '50.00', remaining: '0.00' }),
				member({
					transactionId: 'back',
					role: 'OFFSET',
					amount: '80.00',
					allocated: '50.00',
					remaining: '30.00'
				})
			]
		});
		const hint = linkHint({ id: 'back', currency: 'EUR', link: over })!;
		expect(hint.text).toContain('30.00 EUR surplus');
	});

	it('is null for an unlinked transaction', () => {
		expect(linkHint({ id: 'x', currency: 'EUR', link: null })).toBeNull();
		expect(ownRole({ id: 'x', link: null })).toBeNull();
	});

	it('survives a missing counterpart', () => {
		const orphan = link({
			status: 'INCOMPLETE',
			members: [
				member({
					transactionId: 'back',
					role: 'OFFSET',
					amount: '10.00',
					allocated: '0.00',
					remaining: '10.00'
				})
			]
		});
		expect(linkHint({ id: 'back', currency: 'EUR', link: orphan })!.text).toContain(
			'no longer here'
		);
	});
});

describe('linkabilityOfSelection', () => {
	const row = (id: string, amount: string, currency = 'EUR', l: TransactionLink | null = null) => ({
		id,
		amount,
		currency,
		link: l
	});

	it('splits money out from money in', () => {
		expect(linkabilityOfSelection([row('a', '-10'), row('b', '-20'), row('c', '30')])).toEqual({
			ok: true,
			expenses: ['a', 'b'],
			offsets: ['c']
		});
	});

	it('refuses a single row, one-sided selections, mixed currencies and already-linked rows', () => {
		expect(linkabilityOfSelection([row('a', '-10')])).toMatchObject({ ok: false });
		expect(linkabilityOfSelection([row('a', '-10'), row('b', '-5')])).toMatchObject({ ok: false });
		expect(linkabilityOfSelection([row('a', '-10'), row('b', '5', 'USD')])).toMatchObject({
			ok: false
		});
		expect(linkabilityOfSelection([row('a', '-10'), row('b', '5', 'EUR', partial)])).toMatchObject({
			ok: false
		});
		expect(linkabilityOfSelection([row('a', '-10'), row('b', '5'), row('z', '0')])).toMatchObject({
			ok: false
		});
	});
});

describe('candidateScore', () => {
	it('prefers the closer amount and the nearer date, within 0..1', () => {
		const exactNear = candidateScore('-1000', '2024-09-10', '1000', '2024-09-20');
		expect(exactNear).toBeGreaterThan(candidateScore('-1000', '2024-09-10', '600', '2024-09-20'));
		expect(exactNear).toBeGreaterThan(candidateScore('-1000', '2024-09-10', '1000', '2025-03-10'));
		expect(exactNear).toBeLessThanOrEqual(1);
	});
});

describe('summaryLine and filter values', () => {
	it('is null when nothing is linked, and states partial counts otherwise', () => {
		expect(summaryLine(null, 'EUR')).toBeNull();
		expect(
			summaryLine(
				{
					expenseTotal: '0',
					reimbursed: '0',
					net: '0',
					linkedCount: 0,
					partiallyReimbursedCount: 0
				},
				'EUR'
			)
		).toBeNull();
		expect(
			summaryLine(
				{
					expenseTotal: '1400.00',
					reimbursed: '1000.00',
					net: '400.00',
					linkedCount: 2,
					partiallyReimbursedCount: 1
				},
				'EUR'
			)
		).toBe('Linked expenses: 1,400.00 EUR paid, 1,000.00 EUR reimbursed, 400.00 EUR net. 1 of 2 only partly.');
	});

	it('maps the panel choice onto the GraphQL enum, with include meaning absent', () => {
		expect(reimbursementFilterValue(undefined)).toBeUndefined();
		expect(reimbursementFilterValue('exclude')).toBe('EXCLUDE');
		expect(reimbursementFilterValue('only')).toBe('ONLY');
		expect(reimbursementChoiceFromParam('exclude')).toBe('exclude');
		expect(reimbursementChoiceFromParam('bogus')).toBeUndefined();
		expect(formatMagnitude('-1234.5', 'EUR')).toBe('1,234.50 EUR');
	});
});

describe('dialog helpers', () => {
	it('puts money out on the expense side and money in on the offsetting side', () => {
		expect(roleForAmount('-0.01')).toBe('EXPENSE');
		expect(roleForAmount('5')).toBe('OFFSET');
	});

	it('builds the mutation input from the members and trims the note', () => {
		const a = dialogMemberFromTransaction({
			id: 'a',
			amount: '-10',
			currency: 'EUR',
			bookingDate: '2024-01-01',
			counterpartyName: 'Shop',
			description: null
		});
		const b = dialogMemberFromTransaction({
			id: 'b',
			amount: '4',
			currency: 'EUR',
			bookingDate: '2024-01-05',
			counterpartyName: null,
			description: 'Back'
		});
		expect(b.label).toBe('Back');
		expect(linkInput([a, b], '  dentist ')).toEqual({
			kind: 'REIMBURSEMENT',
			expenseIds: ['a'],
			offsetIds: ['b'],
			note: 'dentist'
		});
		expect(linkInput([a, b], '  ').note).toBeNull();
	});

	it('says what is missing before a link can be saved', () => {
		const out = {
			id: 'a',
			role: 'EXPENSE',
			amount: '-1',
			currency: 'EUR',
			bookingDate: null,
			label: 'x'
		} as const;
		const back = { ...out, id: 'b', role: 'OFFSET', amount: '1' } as const;
		expect(saveBlocker([out])).toContain('reimbursement');
		expect(saveBlocker([back])).toContain('expense');
		expect(saveBlocker([out, { ...back, currency: 'USD' }])).toContain('currency');
		expect(saveBlocker([out, back])).toBeNull();
	});

	it('drops a link member the server could not find', () => {
		expect(dialogMemberFromLink(member({ amount: null }))).toBeNull();
		expect(dialogMemberFromLink(member({ transactionId: 'z' }))!.id).toBe('z');
	});
});
