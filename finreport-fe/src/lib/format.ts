/**
 * Amount/date display formatting — pure helpers, covered by vitest.
 * Accessibility (§6): amounts always carry a textual sign, never colour alone.
 */

/** Format a `Decimal` string with an explicit "+"/"-" sign and a currency code. */
export function formatAmount(value: string, currency: string): string {
	const n = Number(value);
	if (Number.isNaN(n)) return `${value} ${currency}`;
	const sign = n > 0 ? '+' : n < 0 ? '\u2212' : '';
	const abs = Math.abs(n).toLocaleString('en-US', {
		minimumFractionDigits: 2,
		maximumFractionDigits: 2
	});
	return `${sign}${abs} ${currency}`;
}

/** "YYYY-MM-DD" -> "5 Jan 2026" for display in tables/cards. */
export function formatDisplayDate(value: string): string {
	const [y, m, d] = value.split('-').map(Number);
	const date = new Date(y, m - 1, d);
	return date.toLocaleDateString('en-GB', { day: 'numeric', month: 'short', year: 'numeric' });
}
