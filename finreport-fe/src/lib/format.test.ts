import { describe, expect, it } from 'vitest';
import { formatAmount, formatDisplayDate } from './format';

describe('formatAmount', () => {
	it('prefixes a positive amount with +', () => {
		expect(formatAmount('1550.00', 'EUR')).toBe('+1,550.00 EUR');
	});

	it('prefixes a negative amount with a minus sign', () => {
		expect(formatAmount('-950.00', 'EUR')).toBe('\u2212950.00 EUR');
	});

	it('shows no sign for zero', () => {
		expect(formatAmount('0.00', 'EUR')).toBe('0.00 EUR');
	});

	it('falls back for unparseable input', () => {
		expect(formatAmount('n/a', 'EUR')).toBe('n/a EUR');
	});
});

describe('formatDisplayDate', () => {
	it('formats a YYYY-MM-DD date for display', () => {
		expect(formatDisplayDate('2026-01-05')).toBe('5 Jan 2026');
	});
});
