/**
 * Mock `categoryComparison` for `PUBLIC_USE_MOCKS=1`. Unlike the fixed JSON
 * mocks this honours the requested range, so any window the page asks for gets
 * realistic months. Figures are a function of "months ago" so the interesting
 * cases are always near the present, whatever today's date is:
 *
 * - `gym` and `courses` stop (gone), `travel` starts (new), `courses` is the
 *   one that is new in a September-ending view;
 * - `health` has a fully reimbursed bill (nets to 0 with two transactions) and
 *   a partly reimbursed one;
 * - `food` and `dining` move gradually, `rent` never does;
 * - a little unlabelled spending, and a held-for-review amount in the last
 *   two months that must stay out of the totals.
 *
 * The current month is a month-to-date, so its figures are scaled down.
 */

import { parseDateInputValue, toDateInputValue } from '$lib/period';
import type { CategoryComparison, ComparisonCell } from '$lib/comparisonView';

interface MockCategory {
	id: string;
	slug: string;
	name: string;
	/** `[amount, transactionCount]` for `m` months ago, or `null` when absent. */
	at: (m: number) => [number, number] | null;
}

const money = (n: number) => n.toFixed(2);

const CATEGORIES: MockCategory[] = [
	{
		id: '7f5a7e2e-7d0a-5f3d-9c8a-2f1b6a0c4d11',
		slug: 'food',
		name: 'Food',
		at: (m) => [330 + ((m * 7) % 45), 11 + (m % 4)]
	},
	{
		id: 'c3a1b2d4-0000-5000-8000-0000000000a1',
		slug: 'housing',
		name: 'Housing',
		at: () => [950, 1]
	},
	{
		id: 'c3a1b2d4-0000-5000-8000-0000000000a2',
		slug: 'dining',
		name: 'Dining out',
		at: (m) => [90 + (12 - Math.min(m, 12)) * 9, 4 + (m % 3)]
	},
	{
		id: '9a2c3d4e-5f60-5718-8293-a4b5c6d7e8f9',
		slug: 'transportation',
		name: 'Transportation',
		at: (m) => [60 + ((m * 13) % 35), 3 + (m % 2)]
	},
	{
		id: 'c3a1b2d4-0000-5000-8000-0000000000a3',
		slug: 'gym',
		name: 'Gym',
		at: (m) => (m >= 1 ? [29.9, 1] : null)
	},
	{
		id: 'c3a1b2d4-0000-5000-8000-0000000000a4',
		slug: 'travel',
		name: 'Travel',
		at: (m) => (m === 0 ? [480, 3] : m === 4 ? [820, 5] : null)
	},
	{
		id: 'c3a1b2d4-0000-5000-8000-0000000000a5',
		slug: 'courses',
		name: 'Courses',
		at: (m) => (m === 1 ? [199, 1] : null)
	},
	{
		id: 'c3a1b2d4-0000-5000-8000-0000000000a6',
		slug: 'health',
		name: 'Health',
		at: (m) => {
			if (m === 2) return [0, 2]; // 600 bill, reimbursed in full
			if (m === 5) return [60, 2]; // 180 bill, 120 reimbursed
			return m % 2 === 0 ? [25, 1] : null;
		}
	}
];

/** Calendar months from `start` to `end` inclusive, clipped to the range. */
function monthlyPeriods(start: string, end: string): { start: string; end: string }[] {
	const first = parseDateInputValue(start);
	const last = parseDateInputValue(end);
	const periods: { start: string; end: string }[] = [];
	for (
		let cursor = new Date(first.getFullYear(), first.getMonth(), 1);
		cursor <= last && periods.length < 60;
		cursor = new Date(cursor.getFullYear(), cursor.getMonth() + 1, 1)
	) {
		const monthEnd = new Date(cursor.getFullYear(), cursor.getMonth() + 1, 0);
		periods.push({
			start: toDateInputValue(cursor < first ? first : cursor),
			end: toDateInputValue(monthEnd > last ? last : monthEnd)
		});
	}
	return periods;
}

export function mockCategoryComparison(
	variables: Record<string, unknown> | undefined,
	today: Date = new Date()
): { categoryComparison: CategoryComparison } {
	const filter = (variables?.filter ?? {}) as { startDate?: string; endDate?: string };
	const start = filter.startDate ?? toDateInputValue(today);
	const end = filter.endDate ?? start;
	const periods = monthlyPeriods(start, end);
	const nowIndex = today.getFullYear() * 12 + today.getMonth();

	const monthsAgo = (p: { start: string }) => {
		const d = parseDateInputValue(p.start);
		return nowIndex - (d.getFullYear() * 12 + d.getMonth());
	};
	// Month-to-date: scale the current month by how much of it has passed.
	const scale = (m: number) => (m === 0 ? today.getDate() / 30 : 1);

	const categories = CATEGORIES.map((c) => {
		const cells: ComparisonCell[] = periods.map((p) => {
			const m = monthsAgo(p);
			const hit = m < 0 ? null : c.at(m);
			if (!hit) return { amount: money(0), transactionCount: 0 };
			const [amount, count] = hit;
			return {
				amount: money(c.slug === 'housing' ? amount : amount * scale(m)),
				transactionCount: count
			};
		});
		const total = cells.reduce((sum, cell) => sum + Number(cell.amount), 0);
		return {
			category: { id: c.id, slug: c.slug, name: c.name, kind: 'EXPENSE' },
			total: money(total),
			cells
		};
	}).filter((c) => c.cells.some((cell) => cell.transactionCount > 0));
	categories.sort((a, b) => Number(b.total) - Number(a.total));

	return {
		categoryComparison: {
			currency: 'EUR',
			periods: periods.map((p, i) => {
				const m = monthsAgo(p);
				const uncategorized = m < 0 ? 0 : (20 + (m % 3) * 10) * scale(m);
				const needsReview = m === 0 ? 60 * scale(0) : m === 1 ? 35 : 0;
				const categorised = categories.reduce((sum, c) => sum + Number(c.cells[i].amount), 0);
				return {
					start: p.start,
					end: p.end,
					total: money(categorised + uncategorized),
					uncategorized: money(uncategorized),
					needsReview: money(needsReview)
				};
			}),
			categories
		}
	};
}
