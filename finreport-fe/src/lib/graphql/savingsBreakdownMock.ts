/**
 * Mock rows for the dashboard's "Savings by category" card (`kind: SAVING`).
 *
 * Kept here rather than in `graphql/mocks/`, which is WP0-owned and whose
 * `mocks.test.ts` validates every JSON file in it as a whole-query fixture.
 *
 * The shape mirrors `category-breakdown.json`: `share` is each row's fraction
 * of the savings total, not of spending, so the three rows sum to 1.
 */
export type SavingsBreakdownRow = {
	category: { id: string; slug: string; name: string; kind: 'SAVING' };
	amount: string;
	transactionCount: number;
	share: number;
};

const ROWS: SavingsBreakdownRow[] = [
	{
		category: {
			id: '6f1d9c2a-0000-4000-8000-0000000000a1',
			slug: 'savings.stocks_etfs',
			name: 'Stocks & ETFs',
			kind: 'SAVING'
		},
		amount: '600.0000',
		transactionCount: 2,
		share: 0.6
	},
	{
		category: {
			id: '6f1d9c2a-0000-4000-8000-0000000000b2',
			slug: 'savings.child_savings',
			name: 'Child Savings',
			kind: 'SAVING'
		},
		amount: '200.0000',
		transactionCount: 1,
		share: 0.2
	},
	{
		category: {
			id: '6f1d9c2a-0000-4000-8000-0000000000c3',
			slug: 'savings',
			name: 'Savings',
			kind: 'SAVING'
		},
		amount: '200.0000',
		transactionCount: 1,
		share: 0.2
	}
];

export const savingsBreakdownMock = {
	categoryBreakdown: {
		rows: ROWS,
		// A saving-kind row is never "uncategorized" or "needs review": both of
		// those buckets are kind-less by construction, and the spending card
		// already reports them.
		uncategorized: null,
		needsReview: null,
		currency: 'EUR'
	}
};

/** Children of an expanded savings row, keyed by the parent slug. */
export const savingsBreakdownChildrenMock: Record<string, unknown> = {
	savings: {
		rows: ROWS.filter((r) => r.category.slug !== 'savings').map((r) => ({ ...r, share: 0.5 })),
		uncategorized: null,
		needsReview: null,
		currency: 'EUR'
	}
};
