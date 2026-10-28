import { describe, expect, it } from 'vitest';
import {
	balanceTone,
	cashAccounts,
	cashEmptyState,
	cashErrorMessage,
	cashTotals,
	deleteAccountWarning,
	emptyCashForm,
	formFromTransaction,
	fromUnits,
	suggestedCurrency,
	toUnits,
	validateAccountForm,
	validateCashForm,
	withRunningBalance
} from './cashView';
import type { Account, Transaction } from './graphql/types';

function tx(id: string, bookingDate: string, amount: string, extra: Partial<Transaction> = {}) {
	return {
		id,
		accountId: 'a1',
		bookingDate,
		amount,
		currency: 'EUR',
		counterpartyName: null,
		description: null,
		...extra
	} as Transaction;
}

function account(id: string, source: string, currency = 'EUR', displayName = id) {
	return { id, source, currency, displayName } as Account;
}

describe('decimal units', () => {
	it('round-trips without float error', () => {
		expect(fromUnits(toUnits('0.1')! + toUnits('0.2')!)).toBe('0.30');
		expect(fromUnits(toUnits('-12.5')!)).toBe('-12.50');
		expect(fromUnits(toUnits('1.2345')!)).toBe('1.2345');
		expect(fromUnits(0n)).toBe('0.00');
	});
	it('rejects non-decimals and over-precise values', () => {
		expect(toUnits('abc')).toBeNull();
		expect(toUnits('1.23456')).toBeNull();
		expect(toUnits('')).toBeNull();
	});
});

describe('withRunningBalance', () => {
	it('is empty for no rows', () => {
		expect(withRunningBalance([])).toEqual([]);
	});
	it('a single row balances to its own amount', () => {
		const [row] = withRunningBalance([tx('1', '2026-09-01', '-5.00')]);
		expect(row.balance).toBe('-5.00');
	});
	it('lists newest first with the balance after each row, chronologically', () => {
		const rows = withRunningBalance([
			tx('3', '2026-09-03', '-30.00'),
			tx('1', '2026-09-01', '100.00'),
			tx('2', '2026-09-02', '-20.00')
		]);
		expect(rows.map((r) => r.transaction.id)).toEqual(['3', '2', '1']);
		expect(rows.map((r) => r.balance)).toEqual(['50.00', '80.00', '100.00']);
	});
	it('goes negative when more is spent than received', () => {
		const rows = withRunningBalance([
			tx('1', '2026-09-01', '10.00'),
			tx('2', '2026-09-02', '-25.50')
		]);
		expect(rows[0].balance).toBe('-15.50');
		expect(balanceTone(rows[0].balance)).toBe('negative');
	});
	it('orders rows on one day by id, deterministically', () => {
		const a = withRunningBalance([tx('b', '2026-09-01', '1.00'), tx('a', '2026-09-01', '2.00')]);
		const b = withRunningBalance([tx('a', '2026-09-01', '2.00'), tx('b', '2026-09-01', '1.00')]);
		expect(a).toEqual(b);
	});
	it('does not mutate its input', () => {
		const input = [tx('2', '2026-09-02', '1.00'), tx('1', '2026-09-01', '1.00')];
		withRunningBalance(input);
		expect(input.map((t) => t.id)).toEqual(['2', '1']);
	});
});

describe('cashTotals', () => {
	it('is all zero for no rows', () => {
		expect(cashTotals([])).toEqual({ count: 0, received: '0.00', spent: '0.00', balance: '0.00' });
	});
	it('sums mixed signs', () => {
		const t = cashTotals([tx('1', 'd', '100.00'), tx('2', 'd', '-30.25'), tx('3', 'd', '-9.75')]);
		expect(t).toEqual({ count: 3, received: '100.00', spent: '40.00', balance: '60.00' });
	});
	it('nets to exactly zero', () => {
		const t = cashTotals([tx('1', 'd', '20.00'), tx('2', 'd', '-20.00')]);
		expect(t.balance).toBe('0.00');
		expect(balanceTone(t.balance)).toBe('zero');
	});
});

describe('accounts and empty states', () => {
	it('keeps only cash accounts, sorted by name', () => {
		const out = cashAccounts([
			account('x', 'comdirect'),
			account('w', 'cash', 'EUR', 'Wallet'),
			account('t', 'cash', 'EUR', 'Tin')
		]);
		expect(out.map((a) => a.id)).toEqual(['t', 'w']);
	});
	it('distinguishes no accounts from no transactions', () => {
		expect(cashEmptyState(0, 0)).toBe('no-accounts');
		expect(cashEmptyState(1, 0)).toBe('no-transactions');
		expect(cashEmptyState(1, 3)).toBeNull();
	});
	it('suggests the existing currency, or none for a caller with no accounts', () => {
		expect(suggestedCurrency([])).toBeNull();
		expect(suggestedCurrency([account('a', 'comdirect', 'CHF')])).toBe('CHF');
	});
});

describe('validateCashForm', () => {
	const base = { ...emptyCashForm('2026-09-10', 'EUR'), amount: '12,50' };

	it('signs spending negative and income positive', () => {
		expect(validateCashForm(base, 'EUR').input?.amount).toBe('-12.50');
		expect(validateCashForm({ ...base, direction: 'received' }, 'EUR').input?.amount).toBe('12.50');
	});
	it('refuses zero, blank, signed and over-precise amounts', () => {
		for (const amount of ['0', '0.00', '', '-5', '1.23456', 'abc']) {
			const r = validateCashForm({ ...base, amount }, 'EUR');
			expect(r.input).toBeNull();
			expect(r.errors.amount).toBeTruthy();
		}
	});
	it('refuses 10^12 and above', () => {
		expect(
			validateCashForm({ ...base, amount: '1000000000000' }, 'EUR').errors.amount
		).toBeTruthy();
		expect(validateCashForm({ ...base, amount: '999999999999.9999' }, 'EUR').input).not.toBeNull();
	});
	it("requires the account's currency", () => {
		const r = validateCashForm({ ...base, currency: 'usd' }, 'EUR');
		expect(r.errors.currency).toContain('EUR');
		expect(validateCashForm({ ...base, currency: 'eur' }, 'EUR').input?.currency).toBe('EUR');
	});
	it('enforces text limits and turns blanks into null', () => {
		expect(
			validateCashForm({ ...base, description: 'x'.repeat(501) }, 'EUR').errors.description
		).toBeTruthy();
		expect(
			validateCashForm({ ...base, counterpartyName: 'x'.repeat(201) }, 'EUR').errors
				.counterpartyName
		).toBeTruthy();
		const ok = validateCashForm(
			{ ...base, description: '  ', counterpartyName: ' Bakery ' },
			'EUR'
		);
		expect(ok.input?.description).toBeNull();
		expect(ok.input?.counterpartyName).toBe('Bakery');
	});
	it('needs a date', () => {
		expect(validateCashForm({ ...base, bookingDate: '' }, 'EUR').errors.bookingDate).toBeTruthy();
	});
});

describe('formFromTransaction (edit is a full replacement)', () => {
	it('pre-fills every field so saving an amount change keeps the text', () => {
		const t = tx('1', '2026-09-02', '-8.50', { description: 'Lunch', counterpartyName: 'Bakery' });
		const form = formFromTransaction(t);
		expect(form).toEqual({
			bookingDate: '2026-09-02',
			amount: '8.50',
			direction: 'spent',
			currency: 'EUR',
			description: 'Lunch',
			counterpartyName: 'Bakery'
		});
		const resubmitted = validateCashForm({ ...form, amount: '9' }, 'EUR').input;
		expect(resubmitted).toMatchObject({
			amount: '-9.00',
			description: 'Lunch',
			counterpartyName: 'Bakery'
		});
	});
	it('reads a positive amount as received and null text as empty', () => {
		const form = formFromTransaction(tx('1', '2026-09-02', '5.00'));
		expect(form.direction).toBe('received');
		expect(form.description).toBe('');
	});
});

describe('validateAccountForm', () => {
	it('trims, bounds the label and uppercases the currency', () => {
		const ok = validateAccountForm('  Wallet ', 'eur');
		expect(ok).toMatchObject({ label: 'Wallet', currency: 'EUR', errors: {} });
		expect(validateAccountForm('', 'EUR').errors.label).toBeTruthy();
		expect(validateAccountForm('x'.repeat(101), 'EUR').errors.label).toBeTruthy();
		expect(validateAccountForm('Wallet', 'EURO').errors.currency).toBeTruthy();
	});
	it('skips the currency when renaming', () => {
		expect(validateAccountForm('Tin', null)).toMatchObject({ currency: null, errors: {} });
	});
});

describe('deleteAccountWarning', () => {
	it('names the label and the row count', () => {
		expect(deleteAccountWarning('Wallet', 5)).toContain('"Wallet"');
		expect(deleteAccountWarning('Wallet', 5)).toContain('5 transactions');
		expect(deleteAccountWarning('Wallet', 1)).toContain('1 transaction ');
		expect(deleteAccountWarning('Wallet', 0)).toContain('no transactions');
		expect(deleteAccountWarning('Wallet', 0)).toContain('cannot be undone');
	});
});

describe('cashErrorMessage', () => {
	it('explains a missing broker', () => {
		expect(cashErrorMessage('KAFKA_UNAVAILABLE', 'x', 'create-account')).toContain('just dev-up');
	});
	it('shows the server validation message verbatim', () => {
		const m = cashErrorMessage('VALIDATION', 'currency must match EUR', 'create-account');
		expect(m).toContain('currency must match EUR');
	});
	it('gives one uniform not-found message', () => {
		const a = cashErrorMessage('NOT_FOUND', 'whatever', 'delete-account');
		expect(a).toContain('no longer exists, or is not yours');
		expect(a).not.toContain('whatever');
	});
	it('falls back to the message, then to a generic line', () => {
		expect(cashErrorMessage(undefined, 'boom', 'save-transaction')).toContain('boom');
		expect(cashErrorMessage(undefined, undefined, 'save-transaction')).toContain('Try again');
	});
});
