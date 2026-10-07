/**
 * Exact-decimal arithmetic for the split editor (§6): GraphQL `Decimal`
 * values travel over the wire as strings, so all of this works in integers
 * scaled by 10^4 via `bigint` — never `number`, which can't represent values
 * like 0.1 + 0.2 exactly and would make "parts sum exactly to the total"
 * unreliable right where it matters most.
 */

const SCALE = 4;
const SCALE_FACTOR = 10n ** BigInt(SCALE);

/**
 * Parse a `Decimal` string (e.g. `"-12.3"`, `"7"`, `"19.99995"`) into an
 * exact integer scaled by 10^4, rounding half-away-from-zero beyond the 4th
 * decimal place. Throws on malformed input (not a plain signed decimal).
 */
export function parseAmount(value: string): bigint {
	const trimmed = value.trim();
	const match = /^(-?)(\d+)(?:\.(\d+))?$/.exec(trimmed);
	if (!match) throw new Error(`invalid decimal amount: "${value}"`);
	const [, sign, intPart, fracPart = ''] = match;
	const keptFrac = (fracPart + '0'.repeat(SCALE)).slice(0, SCALE);
	let scaled = BigInt(intPart) * SCALE_FACTOR + BigInt(keptFrac);
	if (fracPart.length > SCALE) {
		const roundingDigit = fracPart.charCodeAt(SCALE) - 48;
		if (roundingDigit >= 5) scaled += 1n;
	}
	return sign === '-' ? -scaled : scaled;
}

/**
 * Format a scaled integer back into a `Decimal` string with at least
 * `minDecimals` fractional digits (default 2, matching the currency amounts
 * already on the wire); trailing zeros beyond that are trimmed. `-0` never
 * carries a sign.
 */
export function formatAmount(scaled: bigint, minDecimals = 2): string {
	const negative = scaled < 0n;
	const abs = negative ? -scaled : scaled;
	const intPart = abs / SCALE_FACTOR;
	let fracPart = (abs % SCALE_FACTOR).toString().padStart(SCALE, '0');
	while (fracPart.length > minDecimals && fracPart.endsWith('0')) {
		fracPart = fracPart.slice(0, -1);
	}
	const isZero = intPart === 0n && /^0*$/.test(fracPart);
	const sign = negative && !isZero ? '-' : '';
	return fracPart ? `${sign}${intPart}.${fracPart}` : `${sign}${intPart}`;
}

/** Sum of scaled amounts — exact `bigint` addition, no rounding involved. */
export function sumAmounts(values: bigint[]): bigint {
	return values.reduce((acc, v) => acc + v, 0n);
}

/** `total - sum(parts)`; zero means the parts exactly cover the total. */
export function remainder(total: bigint, parts: bigint[]): bigint {
	return total - sumAmounts(parts);
}

export function isZeroAmount(value: bigint): boolean {
	return value === 0n;
}
