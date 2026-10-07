import { describe, expect, it } from 'vitest';
import { formatAmount, isZeroAmount, parseAmount, remainder, sumAmounts } from './splitMath';

describe('parseAmount', () => {
	it('parses a plain integer', () => {
		expect(parseAmount('7')).toBe(70000n);
	});

	it('parses two-decimal currency amounts', () => {
		expect(parseAmount('19.99')).toBe(199900n);
		expect(parseAmount('120.00')).toBe(1200000n);
	});

	it('parses negative amounts, keeping the sign on the scaled value', () => {
		expect(parseAmount('-49.99')).toBe(-499900n);
		expect(parseAmount('-0.0001')).toBe(-1n);
	});

	it('pads fewer than 4 decimal digits', () => {
		expect(parseAmount('1.5')).toBe(15000n);
	});

	it('rounds half-away-from-zero beyond 4 decimal places', () => {
		expect(parseAmount('19.99995')).toBe(199999n + 1n); // 20.0000
		expect(parseAmount('19.99994')).toBe(199999n); // stays 19.9999
		expect(parseAmount('-1.00005')).toBe(-10001n); // -1.0001
	});

	it('rejects malformed input', () => {
		expect(() => parseAmount('abc')).toThrow();
		expect(() => parseAmount('1.2.3')).toThrow();
		expect(() => parseAmount('')).toThrow();
	});
});

describe('formatAmount', () => {
	it('formats with 2 decimals by default', () => {
		expect(formatAmount(199900n)).toBe('19.99');
		expect(formatAmount(1200000n)).toBe('120.00');
	});

	it('keeps a negative sign for negative amounts', () => {
		expect(formatAmount(-499900n)).toBe('-49.99');
	});

	it('never signs zero', () => {
		expect(formatAmount(0n)).toBe('0.00');
		expect(formatAmount(-0n)).toBe('0.00');
	});

	it('preserves precision beyond 2 decimals instead of truncating', () => {
		expect(formatAmount(-1n)).toBe('-0.0001');
	});

	it('round-trips through parseAmount', () => {
		for (const value of ['19.99', '-49.99', '0.00', '120.00', '-0.0001']) {
			expect(formatAmount(parseAmount(value))).toBe(value);
		}
	});
});

describe('sumAmounts / remainder', () => {
	it('sums exactly, including negative parts', () => {
		expect(sumAmounts([parseAmount('30.00'), parseAmount('19.99')])).toBe(parseAmount('49.99'));
		expect(sumAmounts([parseAmount('-30.00'), parseAmount('-19.99')])).toBe(parseAmount('-49.99'));
	});

	it('computes a zero remainder when parts exactly cover the total', () => {
		const total = parseAmount('-49.99');
		const parts = [parseAmount('-30.00'), parseAmount('-19.99')];
		expect(isZeroAmount(remainder(total, parts))).toBe(true);
	});

	it('computes a non-zero remainder down to the 4th decimal', () => {
		const total = parseAmount('49.99');
		const parts = [parseAmount('30.00'), parseAmount('19.98')];
		expect(remainder(total, parts)).toBe(parseAmount('0.01'));
		expect(isZeroAmount(remainder(total, parts))).toBe(false);
	});

	it('catches a mismatch hidden by naive float rounding (0.1 + 0.2 case)', () => {
		const total = parseAmount('0.3');
		const parts = [parseAmount('0.1'), parseAmount('0.2')];
		// With floats, 0.1 + 0.2 !== 0.3; bigint arithmetic must still see
		// these as exactly equal.
		expect(isZeroAmount(remainder(total, parts))).toBe(true);
	});

	it('sums an empty parts list to zero', () => {
		expect(sumAmounts([])).toBe(0n);
		expect(isZeroAmount(remainder(parseAmount('0'), []))).toBe(true);
	});
});
