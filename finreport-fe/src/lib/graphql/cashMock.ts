/**
 * Mock for `/admin/cash`, served from `graphqlBackend.ts` in mock mode.
 *
 * Stateful within a server session (module state, like `savingsMock.ts`): a
 * created account or row shows up in the next `Accounts` / `Transactions`
 * query, an edit replaces the row, a delete removes it. It resets when the
 * dev server restarts. All data is synthetic.
 *
 * It models the refusals the page has to render: a new account's currency
 * must match the existing accounts (`MOCK_BANK_CURRENCY`), a transaction's
 * currency must match its account's, zero is refused, and an unknown id is a
 * uniform `NOT_FOUND`. It does not model `KAFKA_UNAVAILABLE`.
 */

/** The frozen mock bank accounts are all EUR, so a new cash account must be too. */
const MOCK_BANK_CURRENCY = 'EUR';

interface MockCashAccount {
	id: string;
	label: string;
	currency: string;
}

interface MockCashTransaction {
	id: string;
	accountId: string;
	bookingDate: string;
	amount: string;
	currency: string;
	counterpartyName: string | null;
	description: string | null;
	/**
	 * Set on the seeded ATM row so the ledger's "Linked as a transfer" marker
	 * is reviewable in mock mode. The real pairing is made asynchronously by
	 * the transfer detector on its next pass — there is no mutation for it, so
	 * a mock that never set it would leave the marker impossible to see.
	 */
	pairedWithBankLeg?: boolean;
}

const WALLET_ID = 'c0a5b000-0000-4000-8000-000000000001';

let accounts: MockCashAccount[] = [{ id: WALLET_ID, label: 'Wallet', currency: 'EUR' }];

let transactions: MockCashTransaction[] = [
	// The case the user described: money leaves the bank account, this row is
	// entered by hand, and the detector links the two.
	{ ...tx('1', '2026-09-01', '200.00', 'Cash withdrawal', 'ATM'), pairedWithBankLeg: true },
	tx('2', '2026-09-03', '-23.40', 'Weekly market', 'Farmers market'),
	tx('3', '2026-09-07', '-8.50', 'Lunch', 'Corner bakery'),
	tx('4', '2026-09-12', '-60.00', 'Haircut and tip', null),
	tx('5', '2026-09-20', '25.00', 'Paid back for tickets', 'Sam')
];

function tx(
	n: string,
	bookingDate: string,
	amount: string,
	description: string | null,
	counterpartyName: string | null
): MockCashTransaction {
	return {
		id: `c0a5b000-0000-4000-8000-0000000001${n.padStart(2, '0')}`,
		accountId: WALLET_ID,
		bookingDate,
		amount,
		currency: 'EUR',
		counterpartyName,
		description
	};
}

let counter = 100;
function nextId(prefix: string): string {
	counter += 1;
	return `c0a5b000-0000-4000-8000-${prefix}${String(counter).padStart(11, '0')}`;
}

type Envelope =
	| { data: unknown }
	| { data: null; errors: { message: string; extensions: object }[] };

function refusal(code: string, message: string): Envelope {
	return { data: null, errors: [{ message, extensions: { code } }] };
}

const NOT_FOUND = () => refusal('NOT_FOUND', 'not found');

function accountShape(a: MockCashAccount) {
	return {
		id: a.id,
		source: 'cash',
		externalId: a.id,
		displayId: null,
		accountType: 'Cash',
		iban: null,
		bic: null,
		institute: null,
		label: a.label,
		displayName: a.label,
		currency: a.currency,
		latestBalance: null
	};
}

function transactionShape(t: MockCashTransaction) {
	return {
		id: t.id,
		accountId: t.accountId,
		source: 'cash',
		externalId: t.id,
		bookingDate: t.bookingDate,
		valutaDate: null,
		bookingStatus: 'BOOKED',
		amount: t.amount,
		currency: t.currency,
		counterpartyName: t.counterpartyName,
		counterpartyDisplayName: t.counterpartyName,
		counterpartyIban: null,
		description: t.description,
		transactionType: null,
		label: null,
		splits: [],
		tags: [],
		note: null,
		// A cash row has no IBAN, so a real pair can only ever be `AMOUNT_DATE`.
		transfer: t.pairedWithBankLeg
			? {
					counterpartTransactionId: 'c0a5b000-0000-4000-8000-00000000bank',
					counterpartAccountId: 'c0a5b000-0000-4000-8000-00000000acct',
					match: 'AMOUNT_DATE'
				}
			: null,
		recurring: {
			isRecurring: false,
			source: 'AUTO',
			seriesId: null,
			cadence: null,
			medianAmount: null
		},
		link: null
	};
}

/** The cash accounts as `Account` rows, to append to the `Accounts` result (without `isSavings`). */
export function mockCashAccountRows() {
	return accounts.map((a) => ({ ...accountShape(a), isSavings: false }));
}

/**
 * The `Transactions` result when the filter names cash accounts, else `null`
 * (the caller serves its own fixtures). A filter naming only cash accounts
 * gets only their rows.
 */
export function mockCashTransactionsPage(variables: Record<string, unknown> | undefined): unknown {
	const ids = (variables?.filter as { accountIds?: unknown[] } | undefined)?.accountIds;
	if (!Array.isArray(ids) || ids.length === 0) return null;
	const cashIds = new Set(accounts.map((a) => a.id));
	if (!ids.every((id) => cashIds.has(String(id)))) return null;
	const wanted = new Set(ids.map(String));
	const rows = transactions
		.filter((t) => wanted.has(t.accountId))
		.sort((a, b) => b.bookingDate.localeCompare(a.bookingDate));
	const page = (variables?.page ?? {}) as { limit?: number; offset?: number };
	const limit = page.limit ?? 50;
	const offset = page.offset ?? 0;
	return {
		transactions: {
			items: rows.slice(offset, offset + limit).map(transactionShape),
			totalCount: rows.length,
			limit,
			offset
		}
	};
}

interface MockInput {
	bookingDate?: string;
	amount?: string;
	currency?: string;
	description?: string | null;
	counterpartyName?: string | null;
}

/** Why the input is refused, in the server's value-free style, or null. */
function inputProblem(input: MockInput, account: MockCashAccount): string | null {
	const amount = Number(input.amount);
	if (!input.bookingDate || !/^\d{4}-\d{2}-\d{2}$/.test(input.bookingDate))
		return 'bookingDate must be a date';
	if (!Number.isFinite(amount) || amount === 0) return 'amount must be non-zero';
	if ((input.description ?? '').length > 500) return 'description is too long (at most 500)';
	if ((input.counterpartyName ?? '').length > 200)
		return 'counterpartyName is too long (at most 200)';
	if (input.currency !== account.currency)
		return `currency must equal the account's currency (${account.currency})`;
	return null;
}

function blankToNull(value: string | null | undefined): string | null {
	return value && value.trim() !== '' ? value.trim() : null;
}

/** Serve one of the six cash mutations, or `null` if `name` is not one. */
export function mockCashMutation(
	name: string,
	variables: Record<string, unknown> | undefined
): Envelope | null {
	const v = variables ?? {};
	switch (name) {
		case 'CreateCashAccount': {
			const label = String(v.label ?? '').trim();
			const currency = String(v.currency ?? '');
			if (label === '' || label.length > 100)
				return refusal('VALIDATION', 'label must be 1 to 100 characters');
			if (currency !== MOCK_BANK_CURRENCY)
				return refusal(
					'VALIDATION',
					`currency must match your existing accounts (${MOCK_BANK_CURRENCY})`
				);
			const account = { id: nextId('a'), label, currency };
			accounts = [...accounts, account];
			return { data: { createCashAccount: accountShape(account) } };
		}
		case 'RenameCashAccount': {
			const account = accounts.find((a) => a.id === v.accountId);
			if (!account) return NOT_FOUND();
			const label = String(v.label ?? '').trim();
			if (label === '' || label.length > 100)
				return refusal('VALIDATION', 'label must be 1 to 100 characters');
			account.label = label;
			return { data: { renameCashAccount: accountShape(account) } };
		}
		case 'DeleteCashAccount': {
			if (!accounts.some((a) => a.id === v.accountId)) return NOT_FOUND();
			accounts = accounts.filter((a) => a.id !== v.accountId);
			transactions = transactions.filter((t) => t.accountId !== v.accountId);
			return { data: { deleteCashAccount: true } };
		}
		case 'CreateCashTransaction': {
			const account = accounts.find((a) => a.id === v.accountId);
			if (!account) return NOT_FOUND();
			const input = (v.input ?? {}) as MockInput;
			const problem = inputProblem(input, account);
			if (problem) return refusal('VALIDATION', problem);
			const row: MockCashTransaction = {
				id: nextId('b'),
				accountId: account.id,
				bookingDate: input.bookingDate as string,
				amount: String(input.amount),
				currency: account.currency,
				counterpartyName: blankToNull(input.counterpartyName),
				description: blankToNull(input.description)
			};
			transactions = [...transactions, row];
			return { data: { createCashTransaction: transactionShape(row) } };
		}
		case 'UpdateCashTransaction': {
			const row = transactions.find((t) => t.id === v.transactionId);
			const account = row && accounts.find((a) => a.id === row.accountId);
			if (!row || !account) return NOT_FOUND();
			const input = (v.input ?? {}) as MockInput;
			const problem = inputProblem(input, account);
			if (problem) return refusal('VALIDATION', problem);
			// A full replacement, as on the server: an omitted optional is cleared.
			row.bookingDate = input.bookingDate as string;
			row.amount = String(input.amount);
			row.counterpartyName = blankToNull(input.counterpartyName);
			row.description = blankToNull(input.description);
			return { data: { updateCashTransaction: transactionShape(row) } };
		}
		case 'DeleteCashTransaction': {
			if (!transactions.some((t) => t.id === v.transactionId)) return NOT_FOUND();
			transactions = transactions.filter((t) => t.id !== v.transactionId);
			return { data: { deleteCashTransaction: true } };
		}
		default:
			return null;
	}
}
