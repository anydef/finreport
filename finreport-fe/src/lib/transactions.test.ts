import { describe, expect, it } from 'vitest';
import {
	accountDisplayId,
	defaultDateRange,
	defaultDateRangeFrom,
	formatAmount,
	pickCounterparty,
	toDateInputValue,
	type Account
} from './transactions';

describe('toDateInputValue', () => {
	it('formats a date as YYYY-MM-DD using local date parts', () => {
		expect(toDateInputValue(new Date(2026, 6, 17))).toBe('2026-07-17');
	});

	it('pads single-digit months and days', () => {
		expect(toDateInputValue(new Date(2026, 0, 5))).toBe('2026-01-05');
	});
});

describe('defaultDateRange', () => {
	it('returns the first day of the month as start and today as end', () => {
		const today = new Date(2026, 6, 17);
		expect(defaultDateRange(today)).toEqual({ start: '2026-07-01', end: '2026-07-17' });
	});

	it('handles the first day of the month correctly', () => {
		const today = new Date(2026, 0, 1);
		expect(defaultDateRange(today)).toEqual({ start: '2026-01-01', end: '2026-01-01' });
	});
});

describe('defaultDateRangeFrom', () => {
	it('anchors end on lastSeen and start 60 days earlier', () => {
		expect(defaultDateRangeFrom('2026-05-10', new Date(2026, 6, 17))).toEqual({
			start: '2026-03-11',
			end: '2026-05-10'
		});
	});

	it('handles a month/year boundary (60 days back crosses into the prior year)', () => {
		expect(defaultDateRangeFrom('2026-01-15', new Date(2026, 6, 17))).toEqual({
			start: '2025-11-16',
			end: '2026-01-15'
		});
	});

	it('falls back to defaultDateRange when lastSeen is null', () => {
		const today = new Date(2026, 6, 17);
		expect(defaultDateRangeFrom(null, today)).toEqual(defaultDateRange(today));
	});

	it('parses lastSeen as a local date, not UTC, avoiding an off-by-one shift', () => {
		// If "YYYY-MM-DD" were parsed via `new Date(string)` it would be
		// interpreted as UTC midnight, which can shift a day backwards when
		// rendered in a timezone behind UTC. Parsing manually into
		// year/month/day components (as this function does) sidesteps that
		// entirely, so the result is stable regardless of the host timezone.
		const result = defaultDateRangeFrom('2026-01-01', new Date(2026, 6, 17));
		expect(result.end).toBe('2026-01-01');
	});
});

describe('pickCounterparty', () => {
	it('prefers creditor when present', () => {
		expect(pickCounterparty({ creditor: 'Acme Corp', remitter: 'Jane', deptor: 'John' })).toBe(
			'Acme Corp'
		);
	});

	it('falls back to remitter when creditor is empty', () => {
		expect(pickCounterparty({ creditor: '', remitter: 'Jane', deptor: 'John' })).toBe('Jane');
	});

	it('falls back to deptor when creditor and remitter are empty', () => {
		expect(pickCounterparty({ creditor: '', remitter: '', deptor: 'John' })).toBe('John');
	});

	it('returns an empty string when nothing is set', () => {
		expect(pickCounterparty({ creditor: '', remitter: '', deptor: '' })).toBe('');
	});

	it('handles missing/undefined fields', () => {
		expect(pickCounterparty({})).toBe('');
	});
});

describe('formatAmount', () => {
	it('formats a numeric string to 2 decimals', () => {
		expect(formatAmount('12.5')).toBe('12.50');
	});

	it('formats a number to 2 decimals', () => {
		expect(formatAmount(-42)).toBe('-42.00');
	});

	it('rounds to 2 decimal places', () => {
		expect(formatAmount('3.14159')).toBe('3.14');
	});

	it('returns the original value when not parseable', () => {
		expect(formatAmount('not-a-number')).toBe('not-a-number');
	});
});

describe('accountDisplayId', () => {
	const accounts: Account[] = [
		{
			id: '1',
			accountId: 'acc-123',
			displayId: 'Checking ****1234',
			accountType: 'CHECKING',
			iban: 'DE00',
			bic: 'BIC',
			institute: 'Bank'
		}
	];

	it('resolves the displayId for a known accountId', () => {
		expect(accountDisplayId('acc-123', accounts)).toBe('Checking ****1234');
	});

	it('falls back to the raw accountId when not found', () => {
		expect(accountDisplayId('unknown-id', accounts)).toBe('unknown-id');
	});
});
