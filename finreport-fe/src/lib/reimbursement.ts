/**
 * Reimbursement links, the pure half: what a link says about a transaction,
 * what a would-be link would amount to, and whether a selection can be linked
 * at all. The server derives every figure from the members' own amounts; the
 * preview here mirrors its arithmetic (offset = min(offsets, expenses), the
 * excess is a surplus) so the dialog can show the outcome before saving.
 *
 * Money is handled in integer cents so a sum never drifts the way floats do.
 */
import { formatDisplayDate } from '$lib/format';
import type {
	ReimbursementSummary,
	Transaction,
	TransactionLink,
	TransactionLinkMember,
	TransactionLinkRole,
	TransactionLinkStatus
} from '$lib/graphql/types';

const toCents = (value: string): number => Math.round(Number(value) * 100);
const fromCents = (cents: number): string => (cents / 100).toFixed(2);

/** An amount without a sign: "1,000.00 EUR". */
export function formatMagnitude(value: string, currency: string): string {
	const n = Math.abs(Number(value));
	if (Number.isNaN(n)) return `${value} ${currency}`;
	return `${n.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 })} ${currency}`;
}

export interface LinkPreview {
	expenseTotal: string;
	offsetTotal: string;
	reimbursed: string;
	net: string;
	surplus: string;
	status: TransactionLinkStatus;
}

/** What linking these amounts would come to, matching the server's arithmetic. */
export function previewOffsets(expenseAmounts: string[], offsetAmounts: string[]): LinkPreview {
	const sum = (xs: string[]) => xs.reduce((acc, x) => acc + Math.abs(toCents(x)), 0);
	const expense = sum(expenseAmounts);
	const offset = sum(offsetAmounts);
	const complete = expense > 0 && offset > 0;
	const reimbursed = complete ? Math.min(expense, offset) : 0;
	const surplus = complete ? Math.max(0, offset - expense) : 0;
	const status: TransactionLinkStatus = !complete
		? 'INCOMPLETE'
		: offset === expense
			? 'FULL'
			: offset < expense
				? 'PARTIAL'
				: 'OVER';
	return {
		expenseTotal: fromCents(expense),
		offsetTotal: fromCents(offset),
		reimbursed: fromCents(reimbursed),
		net: fromCents(expense - reimbursed),
		surplus: fromCents(surplus),
		status
	};
}

export function statusLabel(status: TransactionLinkStatus): string {
	switch (status) {
		case 'FULL':
			return 'Fully reimbursed';
		case 'PARTIAL':
			return 'Partly reimbursed';
		case 'OVER':
			return 'Reimbursed with a surplus';
		case 'INCOMPLETE':
			return 'Incomplete link';
	}
}

/** Which side of its link a transaction is on; `null` when unlinked. */
export function ownRole(tx: Pick<Transaction, 'id' | 'link'>): TransactionLinkRole | null {
	return tx.link?.members.find((m) => m.transactionId === tx.id)?.role ?? null;
}

export interface LinkHint {
	role: TransactionLinkRole;
	status: TransactionLinkStatus;
	/** One line, readable without colour. */
	text: string;
}

function describeMember(m: TransactionLinkMember): string {
	const who = m.counterpartyName ?? m.description ?? 'a removed transaction';
	return m.bookingDate ? `${who}, ${formatDisplayDate(m.bookingDate)}` : who;
}

/**
 * The annotation a linked row carries. An expense says what was paid, what
 * came back and what it still costs ("1,000.00 EUR paid, 600.00 EUR
 * reimbursed, 400.00 EUR net"); a reimbursement names the expense(s) it
 * offsets. A partial reimbursement is stated as such, never rounded to
 * "reimbursed".
 */
export function linkHint(tx: Pick<Transaction, 'id' | 'link' | 'currency'>): LinkHint | null {
	const link = tx.link;
	const role = ownRole(tx);
	if (!link || !role) return null;
	const money = (v: string) => formatMagnitude(v, link.currency || tx.currency);

	if (role === 'EXPENSE') {
		const own = link.members.find((m) => m.transactionId === tx.id)!;
		const paid = money(own.amount ?? '0');
		const back = money(own.allocated);
		const net = money(own.remaining);
		const state =
			link.status === 'INCOMPLETE'
				? 'link incomplete'
				: toCents(own.remaining) === 0
					? 'fully reimbursed'
					: 'partly reimbursed';
		return {
			role,
			status: link.status,
			text: `${paid} paid, ${back} reimbursed, ${net} net (${state})`
		};
	}

	const own = link.members.find((m) => m.transactionId === tx.id)!;
	const expenses = link.members.filter((m) => m.role === 'EXPENSE');
	const [first, ...rest] = expenses;
	const target = first ? describeMember(first) : 'an expense that is no longer here';
	const more = rest.length > 0 ? ` and ${rest.length} more` : '';
	const covers =
		toCents(own.allocated) < Math.abs(toCents(own.amount ?? '0'))
			? ` (${money(own.allocated)} of ${money(own.amount ?? '0')} offsets, ${money(own.remaining)} surplus)`
			: link.status === 'PARTIAL'
				? ` (${money(own.allocated)}, expense only partly covered)`
				: '';
	return { role, status: link.status, text: `Offsets ${target}${more}${covers}` };
}

/** What `/transactions` says next to its totals; `null` when nothing in range is linked. */
export function summaryLine(summary: ReimbursementSummary | null | undefined, currency: string) {
	if (!summary || summary.linkedCount === 0) return null;
	const money = (v: string) => formatMagnitude(v, currency);
	const partial =
		summary.partiallyReimbursedCount > 0
			? ` ${summary.partiallyReimbursedCount} of ${summary.linkedCount} only partly.`
			: '';
	return `Linked expenses: ${money(summary.expenseTotal)} paid, ${money(summary.reimbursed)} reimbursed, ${money(summary.net)} net.${partial}`;
}

export interface Linkable {
	id: string;
	amount: string;
	currency: string;
	link?: TransactionLink | null;
}

export type Linkability =
	| { ok: true; expenses: string[]; offsets: string[] }
	| { ok: false; reason: string };

/**
 * Whether ticked rows can become one link by themselves: money out is the
 * expense side, money in the offsetting side, and both must be present, in
 * one currency, none already on a link.
 */
export function linkabilityOfSelection(rows: Linkable[]): Linkability {
	if (rows.length < 2) {
		return { ok: false, reason: 'Pick the other side of the link.' };
	}
	if (rows.some((r) => r.link)) {
		return { ok: false, reason: 'A selected transaction is already linked. Unlink it first.' };
	}
	if (new Set(rows.map((r) => r.currency)).size > 1) {
		return { ok: false, reason: 'All transactions in a link must share one currency.' };
	}
	const expenses = rows.filter((r) => Number(r.amount) < 0).map((r) => r.id);
	const offsets = rows.filter((r) => Number(r.amount) > 0).map((r) => r.id);
	if (expenses.length === 0 || offsets.length === 0) {
		return { ok: false, reason: 'A link needs money paid out and money paid back.' };
	}
	if (expenses.length + offsets.length !== rows.length) {
		return { ok: false, reason: 'A zero-amount transaction cannot be linked.' };
	}
	return { ok: true, expenses, offsets };
}

/**
 * How well a candidate fits a base transaction, 0..1: closer in amount and
 * nearer in date is better, amount counting for more. The server ranks real
 * results with the same formula; the mock backend uses this one.
 */
export function candidateScore(
	baseAmount: string,
	baseDate: string,
	candidateAmount: string,
	candidateDate: string
): number {
	const a = Math.abs(Number(baseAmount));
	const b = Math.abs(Number(candidateAmount));
	const large = Math.max(a, b);
	const amountFit = large === 0 ? 0 : Math.min(a, b) / large;
	const days = Math.abs(Date.parse(candidateDate) - Date.parse(baseDate)) / 86_400_000;
	return 0.6 * amountFit + 0.4 * (1 / (1 + days / 30));
}

/** The reimbursement filter as the panel holds it: absent = include. */
export type ReimbursementChoice = 'exclude' | 'only' | undefined;

export function reimbursementFilterValue(choice: ReimbursementChoice) {
	return choice === 'exclude' ? 'EXCLUDE' : choice === 'only' ? 'ONLY' : undefined;
}

export function reimbursementChoiceFromParam(raw: string | null): ReimbursementChoice {
	return raw === 'exclude' || raw === 'only' ? raw : undefined;
}

/** One transaction as the link dialog lists it, whichever query it came from. */
export interface DialogMember {
	id: string;
	role: TransactionLinkRole;
	amount: string;
	currency: string;
	bookingDate: string | null;
	label: string;
}

/** Money out is the expense side of a reimbursement, money in the offsetting side. */
export function roleForAmount(amount: string): TransactionLinkRole {
	return Number(amount) < 0 ? 'EXPENSE' : 'OFFSET';
}

type Describable = Pick<
	Transaction,
	'id' | 'amount' | 'currency' | 'bookingDate' | 'counterpartyName' | 'description'
>;

export function dialogMemberFromTransaction(tx: Describable): DialogMember {
	return {
		id: tx.id,
		role: roleForAmount(tx.amount),
		amount: tx.amount,
		currency: tx.currency,
		bookingDate: tx.bookingDate,
		label: tx.counterpartyName ?? tx.description ?? 'Unnamed transaction'
	};
}

/** `null` for a member the server could not find (no amount to show or count). */
export function dialogMemberFromLink(m: TransactionLinkMember): DialogMember | null {
	if (m.amount === null) return null;
	return {
		id: m.transactionId,
		role: m.role,
		amount: m.amount,
		currency: m.currency ?? 'EUR',
		bookingDate: m.bookingDate,
		label: m.counterpartyName ?? m.description ?? 'Unnamed transaction'
	};
}

/** The mutation input for a dialog's members. */
export function linkInput(members: DialogMember[], note: string) {
	return {
		kind: 'REIMBURSEMENT' as const,
		expenseIds: members.filter((m) => m.role === 'EXPENSE').map((m) => m.id),
		offsetIds: members.filter((m) => m.role === 'OFFSET').map((m) => m.id),
		note: note.trim() === '' ? null : note.trim()
	};
}

/** Why the dialog cannot save yet, or `null` when it can. */
export function saveBlocker(members: DialogMember[]): string | null {
	if (!members.some((m) => m.role === 'EXPENSE')) return 'Add the expense that was paid.';
	if (!members.some((m) => m.role === 'OFFSET')) return 'Add the reimbursement that came back.';
	if (new Set(members.map((m) => m.currency)).size > 1) return 'One currency per link.';
	return null;
}
