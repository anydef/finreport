/**
 * Pure shaping for the savings card. No Svelte, no I/O — covered by
 * `savingsView.test.ts`.
 */

import { decimalToNumber } from './chartShaping';
import type { SavingsSummary } from './graphql/types';

export interface SavingsHeadline {
	/** `netPutAside` as a number; negative when the period drew savings down. */
	net: number;
	/** "Put aside" when positive, "Taken out of savings" when negative. */
	label: string;
	/** Reads as the figure's own sentence, not a chart legend. */
	detail: string;
	tone: 'good' | 'bad' | 'neutral';
}

/**
 * The headline figure and the sentence under it.
 *
 * A period can legitimately be negative — drawing savings down is a real
 * outcome, not an error — so the wording changes rather than the sign being
 * hidden behind an absolute value.
 */
export function savingsHeadline(summary: SavingsSummary): SavingsHeadline {
	const net = decimalToNumber(summary.netPutAside);
	const paidIn = decimalToNumber(summary.paidIn);
	const withdrawn = decimalToNumber(summary.withdrawn);
	const label = net < 0 ? 'Taken out of savings' : 'Put aside';
	const detail =
		withdrawn === 0
			? `${fmt(paidIn)} in, nothing taken out.`
			: `${fmt(paidIn)} in, ${fmt(withdrawn)} out.`;
	return { net, label, detail, tone: net > 0 ? 'good' : net < 0 ? 'bad' : 'neutral' };
}

function fmt(n: number): string {
	return n.toLocaleString('en-GB', { minimumFractionDigits: 2, maximumFractionDigits: 2 });
}

/**
 * The note that keeps the two cards honest.
 *
 * Net-balance-change semantics mean a payment made straight from a savings
 * account counts here *and* in the spending card. The figures are correct but
 * they are not a partition, so anyone adding them up would double-count. The
 * card says so, and only when there is actually such a payment.
 */
export function savingsOverlapNote(summary: SavingsSummary): string | null {
	const spent = decimalToNumber(summary.spentDirectly);
	if (spent <= 0) return null;
	return `${fmt(spent)} ${summary.currency} of this was spent straight from a savings account, so it also appears under spending. The two cards overlap by that much.`;
}

/**
 * Savings-to-savings movements change no total, so they are excluded. Saying
 * how many were excluded stops the card looking like it lost a transfer the
 * user can see on their statement.
 */
export function savingsInternalNote(summary: SavingsSummary): string | null {
	const n = summary.internalTransferCount;
	if (n <= 0) return null;
	return n === 1
		? 'One movement between two savings accounts is not counted: it changes no total.'
		: `${n} movements between savings accounts are not counted: they change no total.`;
}
