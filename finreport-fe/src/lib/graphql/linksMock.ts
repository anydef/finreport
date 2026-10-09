/**
 * Mock-mode answers for the reimbursement-link operations. The fixture
 * (`linksFixture.json`) carries three linked groups: a fully
 * reimbursed expense, a partly reimbursed one, and one reimbursement spread
 * across two expenses. The mutations are stateless echoes, like the other
 * mock mutations: they answer with the link the arguments would produce.
 */
import linkFixture from './linksFixture.json';
import { candidateScore, previewOffsets } from '$lib/reimbursement';
import type { TransactionLink, TransactionLinkMember } from './types';

export interface LinkableMock {
	id: string;
	amount: string;
	currency: string;
	bookingDate: string;
	counterpartyName: string | null;
	description: string | null;
	link?: unknown;
}

/** The linked transactions the fixture adds to the base list. */
export function linkFixtureItems(): LinkableMock[] {
	return linkFixture.data.transactions.items as unknown as LinkableMock[];
}

function linkOf(item: LinkableMock): TransactionLink | null {
	return (item.link as TransactionLink | null | undefined) ?? null;
}

function ownMember(item: LinkableMock): TransactionLinkMember | undefined {
	return linkOf(item)?.members.find((m) => m.transactionId === item.id);
}

/** Is this row the offsetting side of a reimbursement link? */
export function isReimbursementOffset(item: LinkableMock): boolean {
	return ownMember(item)?.role === 'OFFSET';
}

/** `reimbursements` filter: absent/`INCLUDE` keeps everything. */
export function applyReimbursementFilter<T extends LinkableMock>(
	items: T[],
	mode: string | undefined
): T[] {
	if (mode === 'EXCLUDE') return items.filter((i) => !isReimbursementOffset(i));
	if (mode === 'ONLY') return items.filter((i) => isReimbursementOffset(i));
	return items;
}

const cents = (v: string) => Math.round(Math.abs(Number(v)) * 100);
const money = (c: number) => (c / 100).toFixed(2);

/** Pro-rata shares of `total` cents over `weights`, summing exactly (remainder to the last). */
function share(total: number, weights: number[]): number[] {
	const sum = weights.reduce((a, b) => a + b, 0);
	if (sum === 0) return weights.map(() => 0);
	const parts = weights.map((w) => Math.floor((total * w) / sum));
	let left = total - parts.reduce((a, b) => a + b, 0);
	for (let i = parts.length - 1; left > 0 && i >= 0; i--, left--) parts[i] += 1;
	return parts;
}

/** The link `createTransactionLink` / `updateTransactionLink` would answer with. */
export function mockSaveLink(
	variables: Record<string, unknown> | undefined,
	items: LinkableMock[],
	existingId?: string
): TransactionLink {
	const input = (variables?.input ?? {}) as {
		expenseIds?: string[];
		offsetIds?: string[];
		note?: string | null;
	};
	const find = (id: string) => items.find((i) => i.id === id);
	const side = (ids: string[] | undefined) =>
		(ids ?? []).map(find).filter((i): i is LinkableMock => i !== undefined);
	const expenses = side(input.expenseIds);
	const offsets = side(input.offsetIds);
	const preview = previewOffsets(
		expenses.map((e) => e.amount),
		offsets.map((o) => o.amount)
	);
	const reimbursed = cents(preview.reimbursed);
	const expenseShares = share(reimbursed, expenses.map((e) => cents(e.amount)));
	const offsetShares = share(reimbursed, offsets.map((o) => cents(o.amount)));
	const member = (
		item: LinkableMock,
		role: 'EXPENSE' | 'OFFSET',
		allocated: number
	): TransactionLinkMember => ({
		transactionId: item.id,
		role,
		bookingDate: item.bookingDate,
		amount: item.amount,
		currency: item.currency,
		counterpartyName: item.counterpartyName,
		description: item.description,
		allocated: money(allocated),
		remaining: money(cents(item.amount) - allocated)
	});
	return {
		id: existingId ?? crypto.randomUUID(),
		kind: 'REIMBURSEMENT',
		note: input.note?.trim() ? input.note.trim() : null,
		status: preview.status,
		currency: expenses[0]?.currency ?? offsets[0]?.currency ?? 'EUR',
		expenseTotal: preview.expenseTotal,
		offsetTotal: preview.offsetTotal,
		reimbursed: preview.reimbursed,
		net: preview.net,
		surplus: preview.surplus,
		missingMembers: 0,
		members: [
			...expenses.map((e, i) => member(e, 'EXPENSE', expenseShares[i])),
			...offsets.map((o, i) => member(o, 'OFFSET', offsetShares[i]))
		]
	};
}

/** `linkCandidates`: unlinked transactions of the opposite sign, best fit first. */
export function mockLinkCandidates(
	variables: Record<string, unknown> | undefined,
	items: LinkableMock[]
) {
	const base = items.find((i) => i.id === variables?.transactionId);
	if (!base) return { linkCandidates: [] };
	const search = ((variables?.search as string | null | undefined) ?? '').trim().toLowerCase();
	const limit = (variables?.limit as number | undefined) ?? 20;
	const wantNegative = Number(base.amount) > 0;
	const ranked = items
		.filter((i) => i.id !== base.id && !linkOf(i) && Number(i.amount) < 0 === wantNegative)
		.filter(
			(i) =>
				!search || `${i.counterpartyName ?? ''} ${i.description ?? ''}`.toLowerCase().includes(search)
		)
		.map((i) => ({
			score: candidateScore(base.amount, base.bookingDate, i.amount, i.bookingDate),
			transaction: {
				id: i.id,
				bookingDate: i.bookingDate,
				amount: i.amount,
				currency: i.currency,
				counterpartyName: i.counterpartyName,
				description: i.description
			}
		}))
		.sort((a, b) => b.score - a.score);
	return { linkCandidates: ranked.slice(0, limit) };
}

/** `reimbursementSummary` over the already-filtered rows: only expense-side members count. */
export function mockReimbursementSummary(items: LinkableMock[]) {
	let expense = 0;
	let reimbursed = 0;
	let linked = 0;
	let partial = 0;
	for (const item of items) {
		const own = ownMember(item);
		if (own?.role !== 'EXPENSE') continue;
		linked += 1;
		expense += cents(item.amount);
		reimbursed += cents(own.allocated);
		if (cents(own.remaining) > 0) partial += 1;
	}
	return {
		reimbursementSummary: {
			expenseTotal: money(expense),
			reimbursed: money(reimbursed),
			net: money(expense - reimbursed),
			linkedCount: linked,
			partiallyReimbursedCount: partial
		}
	};
}
