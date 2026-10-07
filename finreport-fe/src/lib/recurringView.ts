/**
 * Pure view-model helpers for `/recurring` (iteration 3 §4/§5): monthly
 * equivalent + total in exact decimal (via `splitMath`'s scaled-`bigint`
 * arithmetic, never `number`), sorting and staleness. The server already
 * computes `monthlyEquivalent`/`totalMonthlyEquivalent` (§3.2/§4) — these
 * helpers let the FE reproduce/verify that arithmetic independently and
 * recompute staleness live (today's date, not the mock's fixed snapshot).
 */

import { formatAmount, parseAmount } from './splitMath';
import type { RecurringCadence, RecurringSeries } from './graphql/types';

/** Cadence length in calendar days, used only for the monthly-equivalent ratio. */
const CADENCE_MONTHS: Record<RecurringCadence, number> = {
	MONTHLY: 1,
	QUARTERLY: 3,
	YEARLY: 12
};

/** Grace window added to a cadence before a series is considered `stale` (§4). */
const CADENCE_GRACE_DAYS: Record<RecurringCadence, number> = {
	MONTHLY: 10,
	QUARTERLY: 20,
	YEARLY: 30
};

/**
 * `medianAmount × {monthly: 1, quarterly: 1/3, yearly: 1/12}` (§3.2), exact
 * decimal, half-up to 4 dp — done as `medianAmount * monthsInCadence / 12`
 * scaled-integer math so the 1/3 and 1/12 ratios never hit floating point.
 */
export function monthlyEquivalent(medianAmount: string, cadence: RecurringCadence): string {
	const scaled = parseAmount(medianAmount);
	const months = BigInt(CADENCE_MONTHS[cadence]);
	// scaled is already at 10^4 precision; multiplying by 12 first keeps the
	// division exact to 4dp with half-up rounding on the remainder.
	const numerator = scaled * 12n;
	const denominator = months * 12n;
	const quotient = numerator / denominator;
	const remainder = numerator % denominator;
	// Half-up rounding away from zero on the fractional remainder.
	const roundedUp =
		(remainder < 0n ? -remainder : remainder) * 2n >=
		(denominator < 0n ? -denominator : denominator)
			? numerator < 0n
				? -1n
				: 1n
			: 0n;
	return formatAmount(quotient + roundedUp, 4);
}

/** Sum of expense-direction (negative) series' monthly-equivalent magnitudes, as an exact decimal string. */
export function totalMonthlyEquivalent(series: RecurringSeries[]): string {
	const total = series
		.filter((s) => s.direction === 'SPENDING')
		.reduce((acc, s) => acc + absScaled(s.monthlyEquivalent), 0n);
	return formatAmount(total, 4);
}

function absScaled(amount: string): bigint {
	const scaled = parseAmount(amount);
	return scaled < 0n ? -scaled : scaled;
}

/** Series sorted by monthly-equivalent magnitude, descending (§5). */
export function sortByMonthlyEquivalentDesc(series: RecurringSeries[]): RecurringSeries[] {
	return [...series].sort((a, b) => {
		const diff = absScaled(b.monthlyEquivalent) - absScaled(a.monthlyEquivalent);
		return diff > 0n ? 1 : diff < 0n ? -1 : 0;
	});
}

/**
 * Whether `nextExpectedDate` is far enough in the past, relative to `today`,
 * that the series looks like it stopped (§3.2 "stale, no alerting"). The
 * grace window scales with cadence so a yearly series isn't flagged for
 * being a week "late".
 */
export function isStale(
	nextExpectedDate: string,
	cadence: RecurringCadence,
	today: Date = new Date()
): boolean {
	const next = new Date(`${nextExpectedDate}T00:00:00Z`);
	const graceMs = CADENCE_GRACE_DAYS[cadence] * 24 * 60 * 60 * 1000;
	return today.getTime() - next.getTime() > graceMs;
}
