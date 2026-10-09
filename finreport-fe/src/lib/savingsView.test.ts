import { describe, expect, it } from 'vitest';
import { savingsHeadline, savingsInternalNote, savingsOverlapNote } from './savingsView';
import type { SavingsSummary } from './graphql/types';

const summary = (over: Partial<SavingsSummary> = {}): SavingsSummary =>
	({
		currency: 'EUR',
		netPutAside: '650.0000',
		paidIn: '900.0000',
		withdrawn: '250.0000',
		accounts: [],
		internalTransferCount: 1,
		spentDirectly: '50.0000',
		...over
	}) as SavingsSummary;

describe('savingsHeadline', () => {
	it('reads as money put aside when the period is positive', () => {
		const h = savingsHeadline(summary());
		expect(h.net).toBe(650);
		expect(h.label).toBe('Put aside');
		expect(h.detail).toBe('900.00 in, 250.00 out.');
		expect(h.tone).toBe('good');
	});

	// Drawing savings down is a real outcome, so the wording changes rather
	// than the sign being hidden.
	it('says so plainly when the period drew savings down', () => {
		const h = savingsHeadline(
			summary({ netPutAside: '-300.0000', paidIn: '100.0000', withdrawn: '400.0000' })
		);
		expect(h.net).toBe(-300);
		expect(h.label).toBe('Taken out of savings');
		expect(h.tone).toBe('bad');
	});

	it('does not mention withdrawals when there were none', () => {
		const h = savingsHeadline(
			summary({ netPutAside: '900.0000', paidIn: '900.0000', withdrawn: '0.0000' })
		);
		expect(h.detail).toBe('900.00 in, nothing taken out.');
		expect(h.tone).toBe('good');
	});

	it('is neutral at exactly zero', () => {
		expect(
			savingsHeadline(summary({ netPutAside: '0.0000', paidIn: '200.0000', withdrawn: '200.0000' }))
				.tone
		).toBe('neutral');
	});
});

describe('savingsOverlapNote', () => {
	// The two cards are each correct but are not a partition; adding them
	// double-counts whatever was spent straight from savings.
	it('discloses the overlap with the spending card', () => {
		expect(savingsOverlapNote(summary())).toContain('50.00 EUR');
		expect(savingsOverlapNote(summary())).toContain('also appears under spending');
	});

	it('says nothing when no money was spent straight from savings', () => {
		expect(savingsOverlapNote(summary({ spentDirectly: '0.0000' }))).toBeNull();
	});
});

describe('savingsInternalNote', () => {
	it('accounts for a single excluded internal movement', () => {
		expect(savingsInternalNote(summary({ internalTransferCount: 1 }))).toBe(
			'One movement between two savings accounts is not counted: it changes no total.'
		);
	});

	it('pluralises and stays silent at zero', () => {
		expect(savingsInternalNote(summary({ internalTransferCount: 3 }))).toContain('3 movements');
		expect(savingsInternalNote(summary({ internalTransferCount: 0 }))).toBeNull();
	});
});
