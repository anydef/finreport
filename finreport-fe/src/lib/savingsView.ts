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
 * account counts here *and* in the spending card. "Includes" says the headline
 * already contains that amount, so nobody adds the two cards up. Shown only
 * when there is actually such a payment.
 */
export function savingsOverlapNote(summary: SavingsSummary): string | null {
	const spent = decimalToNumber(summary.spentDirectly);
	if (spent <= 0) return null;
	return `Includes ${fmt(spent)} ${summary.currency} spent directly from savings, which also counts as spending.`;
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

/** What the dashboard's savings card shows. */
export type SavingsCardState =
	| { kind: 'hidden' }
	| { kind: 'summary'; summary: SavingsSummary }
	| { kind: 'mixed-currency'; message: string };

export const MIXED_CURRENCY_MESSAGE =
	'Your savings accounts are in more than one currency, so they cannot be added into one figure. Narrow the account filter to accounts of a single currency to see it.';

/**
 * Decide what to render from a `savingsSummary` result. Never throws and never
 * asks the dashboard to fail: no summary (nothing marked, or the account
 * filter excludes every marked account) and any unexpected error both hide the
 * card; only the mixed-currency `VALIDATION` error gets an explanation, because
 * the user can act on it.
 */
export function savingsCardState(
	summary: SavingsSummary | null | undefined,
	error: { code?: string | null; message?: string } | null | undefined
): SavingsCardState {
	if (error) {
		const mixed = error.code === 'VALIDATION' && /currenc/i.test(error.message ?? '');
		return mixed ? { kind: 'mixed-currency', message: MIXED_CURRENCY_MESSAGE } : { kind: 'hidden' };
	}
	return summary ? { kind: 'summary', summary } : { kind: 'hidden' };
}
