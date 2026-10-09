import { describe, expect, it } from 'vitest';
import {
	applyReimbursementFilter,
	linkFixtureItems,
	mockLinkCandidates,
	mockReimbursementSummary,
	mockSaveLink
} from './linksMock';
import { linkHint } from '$lib/reimbursement';

const items = linkFixtureItems();
const byName = (name: string) => items.find((i) => i.counterpartyName === name)!;

describe('the link fixture', () => {
	it('covers a full, a partial and a many-to-one link, each seen from both sides', () => {
		const statuses = new Set(items.map((i) => (i.link as { status: string }).status));
		expect(statuses).toEqual(new Set(['FULL', 'PARTIAL']));
		const many = items.filter((i) => (i.link as { note: string }).note === 'Urlaub geteilt');
		expect(many).toHaveLength(3);
		// every row renders a hint from its own side
		for (const item of items) {
			expect(linkHint(item as Parameters<typeof linkHint>[0])).not.toBeNull();
		}
	});

	it('is internally consistent: the allocations add up on each side', () => {
		const seen = new Set<string>();
		for (const item of items) {
			const link = item.link as {
				id: string;
				reimbursed: string;
				members: { role: string; allocated: string }[];
			};
			if (seen.has(link.id)) continue;
			seen.add(link.id);
			for (const role of ['EXPENSE', 'OFFSET']) {
				const sum = link.members
					.filter((m) => m.role === role)
					.reduce((a, m) => a + Math.round(Number(m.allocated) * 100), 0);
				expect(sum).toBe(Math.round(Number(link.reimbursed) * 100));
			}
		}
	});
});

describe('the reimbursement filter', () => {
	it('leaves out only the offsetting side, and ONLY keeps just that', () => {
		expect(applyReimbursementFilter(items, undefined)).toHaveLength(7);
		expect(applyReimbursementFilter(items, 'INCLUDE')).toHaveLength(7);
		const kept = applyReimbursementFilter(items, 'EXCLUDE').map((i) => i.counterpartyName);
		expect(kept).not.toContain('AOK Krankenkasse');
		expect(kept).toContain('Zahnarzt Dr. Mueller');
		expect(applyReimbursementFilter(items, 'ONLY').map((i) => i.counterpartyName).sort()).toEqual([
			'AOK Krankenkasse',
			'Anna Schmidt',
			'Debeka Erstattung'
		]);
	});
});

describe('mock mutations and queries', () => {
	it('summarises linked expenses: gross, reimbursed and net, with partial counted', () => {
		const { reimbursementSummary: s } = mockReimbursementSummary(items);
		expect(s).toEqual({
			expenseTotal: '1800.00',
			reimbursed: '1200.00',
			net: '600.00',
			linkedCount: 4,
			partiallyReimbursedCount: 3
		});
	});

	it('echoes a saved link with the arithmetic the server would do', () => {
		const link = mockSaveLink(
			{
				input: {
					expenseIds: [byName('Ferienhaus Booking').id, byName('Mietwagen Sixt').id],
					offsetIds: [byName('Anna Schmidt').id],
					note: ' trip '
				}
			},
			items
		);
		expect(link.status).toBe('PARTIAL');
		expect(link.note).toBe('trip');
		expect(link.members.filter((m) => m.role === 'EXPENSE').map((m) => m.allocated)).toEqual([
			'150.00',
			'50.00'
		]);
	});

	it('suggests unlinked opposite-sign rows, best first, and honours a search', () => {
		const extra = [
			{ ...byName('Anna Schmidt'), id: 'free-1', link: null, amount: '990.00', bookingDate: '2024-02-20' },
			{ ...byName('Anna Schmidt'), id: 'free-2', link: null, amount: '5.00', bookingDate: '2024-02-21', counterpartyName: 'Kiosk' }
		];
		const base = { ...byName('Praxis Dr. Meyer'), id: 'base', link: null, amount: '-1000.00' };
		const all = [...extra, base];
		const ids = mockLinkCandidates({ transactionId: 'base' }, all).linkCandidates.map(
			(c) => c.transaction.id
		);
		expect(ids).toEqual(['free-1', 'free-2']);
		const found = mockLinkCandidates({ transactionId: 'base', search: 'kiosk' }, all);
		expect(found.linkCandidates.map((c) => c.transaction.id)).toEqual(['free-2']);
	});
});
