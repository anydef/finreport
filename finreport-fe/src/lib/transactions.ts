export interface Account {
	id: string;
	accountId: string;
	displayId: string;
	accountType: string;
	iban: string;
	bic: string;
	institute: string;
}

export interface Transaction {
	reference: string;
	accountId: string;
	bookingStatus: string;
	bookingDate: string;
	amount: string | number;
	remitter: string;
	deptor: string;
	creditor: string;
	creditorId: string;
	creditorMandateId: string;
	remittanceInfo: string;
	transactionType: string;
}

/**
 * Format a date as a "YYYY-MM-DD" string using local date parts (not UTC),
 * matching what <input type="date"> produces/expects.
 */
export function toDateInputValue(date: Date): string {
	const year = date.getFullYear();
	const month = String(date.getMonth() + 1).padStart(2, '0');
	const day = String(date.getDate()).padStart(2, '0');
	return `${year}-${month}-${day}`;
}

/**
 * Default filter range: first day of the current month through today.
 */
export function defaultDateRange(today: Date): { start: string; end: string } {
	const firstOfMonth = new Date(today.getFullYear(), today.getMonth(), 1);
	return {
		start: toDateInputValue(firstOfMonth),
		end: toDateInputValue(today)
	};
}

/**
 * Default filter range anchored on the last-seen transaction date:
 * end = lastSeen, start = 60 days before lastSeen. Falls back to
 * defaultDateRange (first-of-month..today) when lastSeen is null
 * (no transactions in the DB yet).
 */
export function defaultDateRangeFrom(
	lastSeen: string | null,
	today: Date
): { start: string; end: string } {
	if (!lastSeen) return defaultDateRange(today);
	// Parse "YYYY-MM-DD" as a LOCAL date, not UTC (avoid an off-by-one-day
	// shift that `new Date("YYYY-MM-DD")` would introduce).
	const [y, m, d] = lastSeen.split('-').map(Number);
	const end = new Date(y, m - 1, d);
	const start = new Date(y, m - 1, d - 60);
	return { start: toDateInputValue(start), end: toDateInputValue(end) };
}

/**
 * Pick a display counterparty for a transaction: prefer creditor, then
 * remitter, then deptor, falling back to an empty string.
 */
export function pickCounterparty(tx: {
	creditor?: string | null;
	remitter?: string | null;
	deptor?: string | null;
}): string {
	if (tx.creditor) return tx.creditor;
	if (tx.remitter) return tx.remitter;
	if (tx.deptor) return tx.deptor;
	return '';
}

/**
 * Format an amount (string or number) to a fixed 2-decimal string.
 * Returns the original input (stringified) if it can't be parsed as a number.
 */
export function formatAmount(amount: string | number): string {
	const value = typeof amount === 'number' ? amount : Number(amount);
	if (Number.isNaN(value)) return String(amount);
	return value.toFixed(2);
}

/**
 * Look up an account's displayId by its accountId (the string identifier
 * used on transactions), falling back to the raw accountId if not found.
 */
export function accountDisplayId(accountId: string, accounts: Account[]): string {
	const account = accounts.find((a) => a.accountId === accountId);
	return account ? account.displayId : accountId;
}
