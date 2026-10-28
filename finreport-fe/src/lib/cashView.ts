/**
 * Page shaping for `/admin/cash`: hand-kept cash accounts. Pure, so the
 * running balance, the form validation and the error wording are unit-tested.
 *
 * Money is handled as scaled integers (`BigInt`, four decimals, the server's
 * limit), never as floats: a running sum of 0.1 + 0.2 must show 0.30.
 */
import type { Account, CashTransactionInput, Transaction } from '$lib/graphql/types';

const SCALE = 4;
const UNIT = 10n ** BigInt(SCALE);
/** The server refuses an amount at or above 10^12. */
const MAX_UNITS = 10n ** 12n * UNIT;

export const LABEL_MAX = 100;
export const DESCRIPTION_MAX = 500;
export const COUNTERPARTY_MAX = 200;

/** A decimal string ("-12.5", "3") as an integer count of 1/10000ths, or null if it is not one. */
export function toUnits(value: string): bigint | null {
	const m = /^([+-]?)(\d+)(?:\.(\d{1,4}))?$/.exec(value.trim());
	if (!m) return null;
	const units = BigInt(m[2]) * UNIT + BigInt((m[3] ?? '').padEnd(SCALE, '0'));
	return m[1] === '-' ? -units : units;
}

/** Inverse of `toUnits`: at least two decimals, trailing zeros beyond that trimmed. */
export function fromUnits(units: bigint): string {
	const negative = units < 0n;
	const abs = negative ? -units : units;
	const whole = abs / UNIT;
	const frac = (abs % UNIT).toString().padStart(SCALE, '0').replace(/0+$/, '').padEnd(2, '0');
	return `${negative ? '-' : ''}${whole}.${frac}`;
}

export function cashAccounts(accounts: Account[]): Account[] {
	return accounts
		.filter((a) => a.source === 'cash')
		.sort((a, b) => a.displayName.localeCompare(b.displayName));
}

/**
 * The currency to suggest for a new account. The server requires it to match
 * the caller's existing accounts, so suggest theirs; `null` (no accounts at
 * all) means any currency is accepted and the field starts empty.
 */
export function suggestedCurrency(accounts: Account[]): string | null {
	return accounts.find((a) => a.currency)?.currency ?? null;
}

export interface CashRow {
	transaction: Transaction;
	/** Balance of the account immediately after this row, chronologically. */
	balance: string;
	/**
	 * Set when the transfer detector has paired this row with a transaction on
	 * another of the user's accounts — the ATM case: money leaves the bank
	 * account, the user enters the matching cash row, and the detector links
	 * the two on its next pass.
	 *
	 * Worth surfacing rather than leaving implicit. The pairing is what stops
	 * the withdrawal being counted as both spending on the bank account and
	 * income on the wallet, and it happens *asynchronously* — so without a
	 * marker the user has no way to tell a row that has been linked from one
	 * that is still waiting for the next detector pass, and no reason to
	 * believe the feature works at all.
	 */
	transferNote: string | null;
}

/**
 * The ledger's wording for a paired row. `match` is the detector's own ranking
 * bucket: `IBAN` means one side's counterparty IBAN matched the other's
 * account IBAN, `AMOUNT_DATE` means it matched on an exact opposite amount
 * within the date window. **A cash row can only ever be `AMOUNT_DATE`**, since
 * it has no IBAN — so saying so plainly is honest about how the link was made,
 * and about the fact that two identical same-day amounts could in principle
 * have been paired the other way round.
 */
export function transferNote(t: Transaction): string | null {
	if (!t.transfer) return null;
	return t.transfer.match === 'IBAN'
		? 'Linked as a transfer (matched by IBAN)'
		: 'Linked as a transfer (matched by amount and date)';
}

/** Oldest first; rows on one day are ordered by id, so the order is stable though not "time of day". */
function chronological(rows: Transaction[]): Transaction[] {
	return [...rows].sort(
		(a, b) => a.bookingDate.localeCompare(b.bookingDate) || a.id.localeCompare(b.id)
	);
}

/**
 * Rows newest first (as people read a ledger), each carrying the running
 * balance after it. A cash account has no bank balance: it is the sum of its
 * own rows, so the first row's balance is its own amount. Rows whose amount is
 * not a decimal are counted as zero rather than poisoning every later balance.
 */
export function withRunningBalance(rows: Transaction[]): CashRow[] {
	let running = 0n;
	const out = chronological(rows).map((transaction) => {
		running += toUnits(transaction.amount) ?? 0n;
		return {
			transaction,
			balance: fromUnits(running),
			transferNote: transferNote(transaction)
		};
	});
	return out.reverse();
}

export interface CashTotals {
	count: number;
	received: string;
	spent: string;
	/** received - spent, which is the account's balance. */
	balance: string;
}

export function cashTotals(rows: Transaction[]): CashTotals {
	let received = 0n;
	let spent = 0n;
	for (const t of rows) {
		const units = toUnits(t.amount) ?? 0n;
		if (units > 0n) received += units;
		else spent += -units;
	}
	return {
		count: rows.length,
		received: fromUnits(received),
		spent: fromUnits(spent),
		balance: fromUnits(received - spent)
	};
}

export type CashEmptyState = 'no-accounts' | 'no-transactions' | null;

export function cashEmptyState(accountCount: number, rowCount: number): CashEmptyState {
	if (accountCount === 0) return 'no-accounts';
	return rowCount === 0 ? 'no-transactions' : null;
}

export function balanceTone(balance: string): 'positive' | 'negative' | 'zero' {
	const units = toUnits(balance) ?? 0n;
	return units > 0n ? 'positive' : units < 0n ? 'negative' : 'zero';
}

// ---- the add/edit form ----------------------------------------------------

export type Direction = 'spent' | 'received';

export interface CashFormValues {
	bookingDate: string;
	/** Always a positive magnitude as typed; `direction` carries the sign. */
	amount: string;
	direction: Direction;
	currency: string;
	description: string;
	counterpartyName: string;
}

export type CashFormErrors = Partial<Record<keyof CashFormValues, string>>;

export function emptyCashForm(today: string, currency: string): CashFormValues {
	return {
		bookingDate: today,
		amount: '',
		direction: 'spent',
		currency,
		description: '',
		counterpartyName: ''
	};
}

/**
 * Pre-fill from an existing row. `updateCashTransaction` is a full
 * replacement, so every field must come from the row or saving would clear it.
 */
export function formFromTransaction(t: Transaction): CashFormValues {
	const units = toUnits(t.amount) ?? 0n;
	return {
		bookingDate: t.bookingDate,
		amount: fromUnits(units < 0n ? -units : units),
		direction: units < 0n ? 'spent' : 'received',
		currency: t.currency,
		description: t.description ?? '',
		counterpartyName: t.counterpartyName ?? ''
	};
}

/** Accepts "12.50" and "12,50"; at most four decimals; no sign (the direction is a separate control). */
export function parseMagnitude(text: string): bigint | null {
	const trimmed = text.trim();
	if (trimmed.startsWith('-') || trimmed.startsWith('+')) return null;
	return toUnits(trimmed.replace(',', '.'));
}

export function validateCashForm(
	values: CashFormValues,
	accountCurrency: string
): { input: CashTransactionInput | null; errors: CashFormErrors } {
	const errors: CashFormErrors = {};
	if (!/^\d{4}-\d{2}-\d{2}$/.test(values.bookingDate)) errors.bookingDate = 'Pick a date.';

	const magnitude = parseMagnitude(values.amount);
	if (values.amount.trim() === '') errors.amount = 'Enter an amount.';
	else if (magnitude === null)
		errors.amount = 'Use a plain number without a sign, with at most 4 decimals.';
	else if (magnitude === 0n) errors.amount = 'The amount cannot be zero.';
	else if (magnitude >= MAX_UNITS) errors.amount = 'That amount is too large.';

	const currency = values.currency.trim().toUpperCase();
	if (!/^[A-Z]{3}$/.test(currency)) errors.currency = 'Use a 3-letter currency code, e.g. EUR.';
	else if (currency !== accountCurrency)
		errors.currency = `This account is in ${accountCurrency}; a transaction must use the same currency.`;

	if (values.description.length > DESCRIPTION_MAX)
		errors.description = `At most ${DESCRIPTION_MAX} characters.`;
	if (values.counterpartyName.length > COUNTERPARTY_MAX)
		errors.counterpartyName = `At most ${COUNTERPARTY_MAX} characters.`;

	if (Object.keys(errors).length > 0 || magnitude === null) return { input: null, errors };
	const signed = values.direction === 'spent' ? -magnitude : magnitude;
	return {
		errors,
		input: {
			bookingDate: values.bookingDate,
			amount: fromUnits(signed),
			currency,
			description: values.description.trim() === '' ? null : values.description.trim(),
			counterpartyName:
				values.counterpartyName.trim() === '' ? null : values.counterpartyName.trim()
		}
	};
}

/** `currency` is `null` when the field is not being asked for (rename). */
export function validateAccountForm(
	label: string,
	currency: string | null
): { label: string; currency: string | null; errors: { label?: string; currency?: string } } {
	const errors: { label?: string; currency?: string } = {};
	const trimmed = label.trim();
	if (trimmed === '') errors.label = 'Give the account a name.';
	else if (trimmed.length > LABEL_MAX) errors.label = `At most ${LABEL_MAX} characters.`;
	let code: string | null = null;
	if (currency !== null) {
		code = currency.trim().toUpperCase();
		if (!/^[A-Z]{3}$/.test(code)) errors.currency = 'Use a 3-letter currency code, e.g. EUR.';
	}
	return { label: trimmed, currency: code, errors };
}

// ---- destructive confirmations -----------------------------------------------

/** Names what will be lost: the label and how many rows go with it. */
export function deleteAccountWarning(label: string, rowCount: number): string {
	const rows =
		rowCount === 0
			? 'It has no transactions.'
			: `Its ${rowCount === 1 ? '1 transaction' : `${rowCount} transactions`} will be deleted with it.`;
	return `Delete the cash account "${label}" permanently? ${rows} Any transfer link to a bank transaction is undone. This cannot be undone.`;
}

export function deleteTransactionWarning(): string {
	return 'Delete this transaction permanently? If it was linked as a transfer to a bank transaction, that link is undone. This cannot be undone.';
}

// ---- server errors ----------------------------------------------------------

export type CashAction =
	| 'load'
	| 'create-account'
	| 'rename-account'
	| 'delete-account'
	| 'save-transaction'
	| 'delete-transaction';

const ACTION_TEXT: Record<CashAction, string> = {
	load: 'load your cash accounts',
	'create-account': 'create the account',
	'rename-account': 'rename the account',
	'delete-account': 'delete the account',
	'save-transaction': 'save the transaction',
	'delete-transaction': 'delete the transaction'
};

/**
 * User-facing text for a failed call. `NOT_FOUND` is deliberately uniform on
 * the server (unknown, a bank account, someone else's), so it is not
 * distinguished here either. A `VALIDATION` message is the server's own and
 * value-free, so it is shown verbatim.
 */
export function cashErrorMessage(
	code: string | undefined,
	message: string | undefined,
	action: CashAction
): string {
	switch (code) {
		case 'KAFKA_UNAVAILABLE':
			return 'The event log (Kafka) is not configured on this backend, so cash accounts cannot be changed. `just dev-be` runs without a broker; use the full local stack (`just dev-up`).';
		case 'KAFKA_PUBLISH_FAILED':
			return 'The event log did not accept the change, so it may not be stored. Check the list and try again.';
		case 'NOT_FOUND':
			return `Could not ${ACTION_TEXT[action]}: that cash account or transaction no longer exists, or is not yours. Reload the page.`;
		case 'VALIDATION':
			return message ? `Not saved: ${message}` : `Could not ${ACTION_TEXT[action]}: invalid input.`;
		case 'UNAUTHENTICATED':
			return 'Your session has ended. Sign in again.';
		default:
			return message
				? `Could not ${ACTION_TEXT[action]}: ${message}`
				: `Could not ${ACTION_TEXT[action]}. Try again.`;
	}
}
