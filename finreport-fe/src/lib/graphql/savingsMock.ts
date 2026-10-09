/**
 * Mock data for the account-based savings card and the account settings page.
 *
 * Mock-only: there is no backend for `savingsSummary`, `isSavings` or
 * `setAccountSavings` yet. This file exists so the UI can be reviewed before
 * any of it is built, and it encodes the contract that work will implement.
 *
 * Kept out of `graphql/mocks/`, which is WP0-owned and whose `mocks.test.ts`
 * validates every JSON file there as a whole-query fixture.
 *
 * Unlike the frozen JSON fixtures, the savings flag here is **mutable module
 * state**: toggling an account on the settings page changes what the next
 * query returns, so the page behaves like the real thing within one dev
 * session (it resets when the server restarts).
 */

/** The two savings accounts the frozen `accounts.json` does not have. */
export const MOCK_SAVINGS_ACCOUNTS = [
	{
		id: '9c4e1b70-0000-4000-8000-0000000000d1',
		source: 'comdirect',
		externalId: 'MOCK-TAGESGELD',
		displayId: '1234567801',
		accountType: 'Tagesgeld',
		iban: 'DE02120300000000202051',
		bic: 'COBADEFFXXX',
		institute: 'comdirect',
		label: 'Tagesgeld',
		displayName: 'Tagesgeld',
		currency: 'EUR',
		latestBalance: { date: '2024-02-29', amount: '12400.0000', currency: 'EUR' }
	},
	{
		id: '9c4e1b70-0000-4000-8000-0000000000d2',
		source: 'comdirect',
		externalId: 'MOCK-DEPOT',
		displayId: '1234567802',
		accountType: 'Depot',
		iban: 'DE02500105170137075030',
		bic: 'COBADEFFXXX',
		institute: 'comdirect',
		label: 'Depot',
		displayName: 'Depot',
		currency: 'EUR',
		latestBalance: { date: '2024-02-29', amount: '31250.0000', currency: 'EUR' }
	}
];

/** Account id -> marked as savings. Mutated by the mock mutation. */
const savingsFlags = new Map<string, boolean>([
	[MOCK_SAVINGS_ACCOUNTS[0].id, true],
	[MOCK_SAVINGS_ACCOUNTS[1].id, true]
]);

export function isSavingsAccount(accountId: string): boolean {
	return savingsFlags.get(accountId) ?? false;
}

export function setSavingsAccount(accountId: string, isSavings: boolean): void {
	savingsFlags.set(accountId, isSavings);
}

/**
 * The per-account flows behind the card, as individual movements so the
 * summary can be recomputed whenever the flags change - which is what makes
 * toggling an account on the settings page visibly move the figure.
 *
 * `external: true` means the other side is not an account of the user's, so
 * the movement cannot be netted away as internal.
 */
const MOVEMENTS: {
	accountId: string;
	amount: number;
	external: boolean;
	counterpartAccountId?: string;
	note: string;
}[] = [
	{
		accountId: MOCK_SAVINGS_ACCOUNTS[0].id,
		amount: 500,
		external: false,
		counterpartAccountId: '2b0fe9c2-2f4b-5753-95ad-133501e3bd5d',
		note: 'Standing order from the everyday account'
	},
	{
		accountId: MOCK_SAVINGS_ACCOUNTS[1].id,
		amount: 300,
		external: false,
		counterpartAccountId: '2b0fe9c2-2f4b-5753-95ad-133501e3bd5d',
		note: 'ETF purchase funded from the everyday account'
	},
	{
		accountId: MOCK_SAVINGS_ACCOUNTS[0].id,
		amount: -200,
		external: false,
		counterpartAccountId: '2b0fe9c2-2f4b-5753-95ad-133501e3bd5d',
		note: 'Withdrawal back to the everyday account'
	},
	{
		accountId: MOCK_SAVINGS_ACCOUNTS[0].id,
		amount: 100,
		external: true,
		note: 'Interest paid in'
	},
	{
		accountId: MOCK_SAVINGS_ACCOUNTS[1].id,
		amount: -50,
		external: true,
		note: 'Card payment made straight from the depot account'
	},
	// Savings -> savings: nets to zero, counted as neither.
	{
		accountId: MOCK_SAVINGS_ACCOUNTS[0].id,
		amount: -150,
		external: false,
		counterpartAccountId: MOCK_SAVINGS_ACCOUNTS[1].id,
		note: 'Moved between two savings accounts'
	},
	{
		accountId: MOCK_SAVINGS_ACCOUNTS[1].id,
		amount: 150,
		external: false,
		counterpartAccountId: MOCK_SAVINGS_ACCOUNTS[0].id,
		note: 'Moved between two savings accounts'
	}
];

const d = (n: number) => n.toFixed(4);

/** Recompute the summary from the current flags, as the resolver will. */
export function mockSavingsSummary(): unknown {
	const accounts = [...MOCK_SAVINGS_ACCOUNTS, ...TOGGLEABLE_CURRENT_ACCOUNTS].filter((a) =>
		isSavingsAccount(a.id)
	);
	if (accounts.length === 0) return { savingsSummary: null };

	let internalTransferCount = 0;
	const perAccount = accounts.map((account) => {
		let paidIn = 0;
		let withdrawn = 0;
		let spentDirectly = 0;
		for (const m of MOVEMENTS) {
			if (m.accountId !== account.id) continue;
			// Both sides savings: changes no total, so it is neither a
			// contribution nor a withdrawal.
			if (m.counterpartAccountId && isSavingsAccount(m.counterpartAccountId)) {
				internalTransferCount += 1;
				continue;
			}
			if (m.amount >= 0) paidIn += m.amount;
			else {
				withdrawn += -m.amount;
				if (m.external) spentDirectly += -m.amount;
			}
		}
		return { account, paidIn, withdrawn, spentDirectly };
	});

	const paidIn = perAccount.reduce((s, a) => s + a.paidIn, 0);
	const withdrawn = perAccount.reduce((s, a) => s + a.withdrawn, 0);
	const spentDirectly = perAccount.reduce((s, a) => s + a.spentDirectly, 0);

	return {
		savingsSummary: {
			currency: 'EUR',
			netPutAside: d(paidIn - withdrawn),
			paidIn: d(paidIn),
			withdrawn: d(withdrawn),
			// Each internal movement is two rows; report it as one movement.
			internalTransferCount: Math.round(internalTransferCount / 2),
			spentDirectly: d(spentDirectly),
			accounts: perAccount
				.map((a) => ({
					account: { id: a.account.id, displayName: a.account.displayName },
					paidIn: d(a.paidIn),
					withdrawn: d(a.withdrawn),
					net: d(a.paidIn - a.withdrawn)
				}))
				.sort((x, y) => Number(y.net) - Number(x.net))
		}
	};
}

/**
 * The frozen `accounts.json` accounts, so the preview can toggle those too and
 * show what marking a current account as savings does to the figure. They have
 * no movements of their own in `MOVEMENTS`, so turning one on adds a zero row
 * rather than inventing flows.
 */
export const TOGGLEABLE_CURRENT_ACCOUNTS = [
	{
		id: '2b0fe9c2-2f4b-5753-95ad-133501e3bd5d',
		displayName: 'Pavlo - everyday',
		accountType: 'Girokonto'
	},
	{ id: 'dada8a91-51aa-5243-bcf6-12ff5423b400', displayName: 'Joint', accountType: 'Tagesgeld' }
];
