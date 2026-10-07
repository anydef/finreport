/**
 * Pure logic behind the goals pages (iteration 4 §5): bucket shaping,
 * remaining/met arithmetic, progress-bar fractions and the goal form's
 * validation. No Svelte, no network, so it is vitest-covered like
 * `chartShaping.ts`.
 *
 * Money is `Decimal` strings on the wire. All arithmetic here goes through
 * `splitMath`'s scaled-`bigint` helpers — never `number` — and the only
 * numbers produced are chart coordinates and progress-bar fractions, which
 * are display-only.
 */
import { formatAmount as formatScaled, parseAmount } from './splitMath';
import { normalizeTags } from './tags';
import type {
	Goal,
	GoalBucket,
	GoalCadence,
	GoalInput,
	GoalProgress,
	GoalType,
	ScopeCombine,
	Transaction
} from './graphql/types';

const SCALE_FACTOR = 10_000n;
const CENT_STEP = 100n;

/** Whether a bucket is good news, bad news, or not decided yet. */
export type GoalTone = 'good' | 'bad' | 'neutral';

function tryParse(value: string): bigint {
	try {
		return parseAmount(value);
	} catch {
		return 0n;
	}
}

/** Round a scaled (10^4) amount to whole cents, half away from zero. */
function roundToCents(scaled: bigint): bigint {
	const negative = scaled < 0n;
	const abs = negative ? -scaled : scaled;
	const rounded = ((abs + CENT_STEP / 2n) / CENT_STEP) * CENT_STEP;
	return negative ? -rounded : rounded;
}

function groupThousands(intPart: string): string {
	return intPart.replace(/\B(?=(\d{3})+(?!\d))/g, ',');
}

/** "1850.0000" + "EUR" -> "1,850.00 EUR". A negative value keeps a minus sign. */
export function formatMoney(value: string, currency: string): string {
	const text = formatScaled(roundToCents(tryParse(value)), 2);
	const negative = text.startsWith('-');
	const [intPart, frac = '00'] = (negative ? text.slice(1) : text).split('.');
	return `${negative ? '−' : ''}${groupThousands(intPart)}.${frac.padEnd(2, '0')} ${currency}`;
}

// ---------------------------------------------------------------------------
// Labels
// ---------------------------------------------------------------------------

export function goalTypeLabel(type: GoalType): string {
	return type === 'SPENDING_LIMIT' ? 'Spending limit' : 'Saving target';
}

/** What the headline total is called: money spent against a limit, saved toward a target. */
export function totalLabel(type: GoalType): string {
	return type === 'SPENDING_LIMIT' ? 'Spent' : 'Saved';
}

const CADENCE_UNIT: Record<GoalCadence, string> = {
	MONTHLY: 'month',
	QUARTERLY: 'quarter',
	YEARLY: 'year'
};

/** "per month" for a recurring goal, "2026-01-01 to 2026-12-31" for a fixed one. */
export function describeGoalPeriod(goal: Goal): string {
	if (goal.periodKind === 'RECURRING') {
		return goal.cadence ? `per ${CADENCE_UNIT[goal.cadence]}` : 'recurring';
	}
	return `${goal.startDate ?? '?'} to ${goal.endDate ?? 'open-ended'}`;
}

/** The unit an average is over, for "average per month". */
export function periodUnit(goal: Goal): string {
	return goal.periodKind === 'RECURRING' && goal.cadence ? CADENCE_UNIT[goal.cadence] : 'period';
}

// ---------------------------------------------------------------------------
// Arithmetic
// ---------------------------------------------------------------------------

/** `amount - total`, as a Decimal string; negative when over. */
export function remainingAmount(amount: string, total: string): string {
	return formatScaled(tryParse(amount) - tryParse(total), 2);
}

/** `<= amount` for a limit, `>= amount` for a target. */
export function isMet(type: GoalType, amount: string, total: string): boolean {
	const a = tryParse(amount);
	const t = tryParse(total);
	return type === 'SPENDING_LIMIT' ? t <= a : t >= a;
}

/**
 * Progress-bar fill, clamped to `0..1`. A non-positive threshold yields 0
 * (the backend rejects `amount <= 0`, so this is only a divide-by-zero guard).
 */
export function progressFraction(total: string, amount: string): number {
	const a = tryParse(amount);
	const t = tryParse(total);
	if (a <= 0n || t <= 0n) return 0;
	if (t >= a) return 1;
	return Number((t * SCALE_FACTOR) / a) / Number(SCALE_FACTOR);
}

/**
 * Colour decision for a bucket. A bucket still running is never a failure
 * (spec §3.4): a limit already blown is `bad` (it cannot recover), but a
 * target not yet reached is only `neutral`.
 */
export function bucketTone(type: GoalType, amount: string, bucket: GoalBucket): GoalTone {
	const met = isMet(type, amount, bucket.total);
	if (bucket.inProgress) {
		if (type === 'SPENDING_LIMIT') return met ? 'good' : 'bad';
		return met ? 'good' : 'neutral';
	}
	return met ? 'good' : 'bad';
}

/** Tailwind classes per tone, from the shared `@theme` tokens so text and chart colours cannot drift. */
export function toneTextClass(tone: GoalTone): string {
	if (tone === 'good') return 'text-[var(--color-income)]';
	if (tone === 'bad') return 'text-[var(--color-spending)]';
	return 'text-slate-600';
}

export function toneBarClass(tone: GoalTone): string {
	if (tone === 'good') return 'bg-[var(--color-income)]';
	if (tone === 'bad') return 'bg-[var(--color-spending)]';
	return 'bg-[var(--color-muted)]';
}

/** Words for the tone, so status never relies on colour alone. */
export function toneText(type: GoalType, tone: GoalTone, inProgress: boolean): string {
	if (type === 'SPENDING_LIMIT') {
		if (tone === 'bad') return 'Over limit';
		return inProgress ? 'Under limit so far' : 'Under limit';
	}
	if (tone === 'good') return 'Target reached';
	return inProgress ? 'Not there yet' : 'Target missed';
}

/** The bucket a card should headline: the one in progress, else the latest. */
export function currentBucket(buckets: GoalBucket[]): GoalBucket | null {
	if (buckets.length === 0) return null;
	return buckets.find((b) => b.inProgress) ?? buckets[buckets.length - 1];
}

/**
 * Mean `total` over completed buckets only; an in-progress bucket would
 * drag the average down with a half-finished period. `0.00` when there are
 * none yet. Rounded to cents, half away from zero.
 */
export function averageOverCompleted(buckets: GoalBucket[]): string {
	const done = buckets.filter((b) => !b.inProgress);
	if (done.length === 0) return '0.00';
	const sum = done.reduce((acc, b) => acc + tryParse(b.total), 0n);
	const n = BigInt(done.length);
	const half = n / 2n;
	const avg = sum >= 0n ? (sum + half) / n : (sum - half) / n;
	return formatScaled(roundToCents(avg), 2);
}

/** `{ start, end }` covering every bucket, for the unnarrowed transaction list. */
export function progressRange(buckets: GoalBucket[]): { start: string; end: string } | null {
	if (buckets.length === 0) return null;
	return { start: buckets[0].start, end: buckets[buckets.length - 1].end };
}

// ---------------------------------------------------------------------------
// Chart shaping
// ---------------------------------------------------------------------------

export interface GoalBarDatum {
	label: string;
	start: string;
	end: string;
	/** Chart coordinate only; the exact string stays on the bucket. */
	total: number;
	pending: number;
	inProgress: boolean;
	met: boolean;
	tone: GoalTone;
}

function toNumber(scaled: bigint): number {
	return Number(scaled) / Number(SCALE_FACTOR);
}

export function shapeGoalBars(goal: Goal, buckets: GoalBucket[]): GoalBarDatum[] {
	return buckets.map((b) => ({
		label: b.label,
		start: b.start,
		end: b.end,
		total: toNumber(tryParse(b.total)),
		pending: toNumber(tryParse(b.pending)),
		inProgress: b.inProgress,
		met: b.met,
		tone: bucketTone(goal.type, goal.amount, b)
	}));
}

export interface CumulativePoint {
	date: string;
	/** Running total inside the goal range. */
	total: number;
	/** The budget, constant: the line the total is read against. */
	budget: number;
}

/** What one transaction adds to a goal's running total, as a scaled amount (0 = ignored). */
function contribution(type: GoalType, tx: Transaction): bigint {
	// Held-for-review rows are `pending`, never part of the total (spec §3.3).
	if (tx.label?.status === 'NEEDS_REVIEW') return 0n;
	const parts =
		tx.splits.length > 0
			? tx.splits.map((s) => ({ amount: s.amount, kind: s.category.kind }))
			: [{ amount: tx.amount, kind: tx.label?.category?.kind }];
	let sum = 0n;
	for (const part of parts) {
		const amount = tryParse(part.amount);
		if (type === 'SPENDING_LIMIT') {
			sum -= amount; // a refund (positive) reduces the total
		} else if (part.kind === 'SAVING') {
			sum += amount < 0n ? -amount : amount;
		}
	}
	return sum;
}

/**
 * Running total of the goal's matching transactions, one point per booking
 * date, clipped to `[rangeStart, rangeEnd]` and opened with a zero point at
 * `rangeStart`. The rows already arrive scoped from `goalTransactions`; this
 * only accumulates them. The server's bucket total stays the headline number:
 * this line is a shape, not a second source of truth.
 */
export function shapeCumulativeSeries(
	goal: Goal,
	transactions: Transaction[],
	rangeStart: string,
	rangeEnd: string
): CumulativePoint[] {
	const budget = toNumber(tryParse(goal.amount));
	const byDate = new Map<string, bigint>();
	for (const tx of transactions) {
		if (tx.bookingDate < rangeStart || tx.bookingDate > rangeEnd) continue;
		byDate.set(tx.bookingDate, (byDate.get(tx.bookingDate) ?? 0n) + contribution(goal.type, tx));
	}
	const points: CumulativePoint[] = [{ date: rangeStart, total: 0, budget }];
	let running = 0n;
	for (const date of [...byDate.keys()].sort()) {
		running += byDate.get(date)!;
		points.push({ date, total: toNumber(running), budget });
	}
	return points;
}

// ---------------------------------------------------------------------------
// Bucket drill-down (txStart/txEnd search params, as on the dashboard)
// ---------------------------------------------------------------------------

export function bucketRangeParams(bucket: Pick<GoalBucket, 'start' | 'end'>): {
	txStart: string;
	txEnd: string;
} {
	return { txStart: bucket.start, txEnd: bucket.end };
}

/** The bucket whose range exactly matches the narrowed list, if any. */
export function selectedBucket(
	buckets: GoalBucket[],
	txStart: string | null,
	txEnd: string | null
): GoalBucket | null {
	if (!txStart || !txEnd) return null;
	return buckets.find((b) => b.start === txStart && b.end === txEnd) ?? null;
}

/** Totals block for the goal page. Pending is its own field, never summed into `total`. */
export interface GoalTotals {
	totalLabel: string;
	total: string;
	remainingLabel: string;
	remaining: string;
	/** `null` for a fixed goal: one bucket has no "per period" to average. */
	average: string | null;
	pending: string;
}

export function goalTotals(progress: GoalProgress): GoalTotals {
	const { goal } = progress;
	const current = currentBucket(progress.buckets);
	const recurring = goal.periodKind === 'RECURRING';
	return {
		totalLabel: totalLabel(goal.type),
		total: progress.total,
		remainingLabel: recurring ? 'Remaining this period' : 'Remaining',
		remaining: current ? current.remaining : remainingAmount(goal.amount, progress.total),
		average: recurring ? averageOverCompleted(progress.buckets) : null,
		pending: progress.pending
	};
}

// ---------------------------------------------------------------------------
// Goal form
// ---------------------------------------------------------------------------

export interface GoalFormState {
	name: string;
	type: GoalType;
	amount: string;
	categorySlugs: string[];
	tags: string[];
	combine: ScopeCombine;
	tagCombine: ScopeCombine;
	periodKind: 'RECURRING' | 'FIXED';
	cadence: GoalCadence;
	startDate: string;
	endDate: string;
}

export function emptyGoalForm(): GoalFormState {
	return {
		name: '',
		type: 'SPENDING_LIMIT',
		amount: '',
		categorySlugs: [],
		tags: [],
		combine: 'ALL',
		tagCombine: 'ALL',
		periodKind: 'RECURRING',
		cadence: 'MONTHLY',
		startDate: '',
		endDate: ''
	};
}

export function goalToForm(goal: Goal): GoalFormState {
	return {
		name: goal.name,
		type: goal.type,
		amount: formatScaled(tryParse(goal.amount), 2),
		categorySlugs: goal.scope.categories.map((c) => c.slug),
		tags: [...goal.scope.tags],
		combine: goal.scope.combine,
		tagCombine: goal.scope.tagCombine,
		periodKind: goal.periodKind,
		cadence: goal.cadence ?? 'MONTHLY',
		startDate: goal.startDate ?? '',
		endDate: goal.endDate ?? ''
	};
}

export type GoalFormErrors = Partial<
	Record<'name' | 'amount' | 'scope' | 'startDate' | 'endDate', string>
>;

/** Mirrors the server's `GoalInput` rules (§4) so the obvious mistakes never leave the browser. */
export function validateGoalForm(form: GoalFormState): GoalFormErrors {
	const errors: GoalFormErrors = {};
	if (form.name.trim() === '') errors.name = 'Give the goal a name.';
	if (!/^\d+(\.\d{1,4})?$/.test(form.amount.trim()) || tryParse(form.amount.trim()) <= 0n) {
		errors.amount = 'Enter an amount greater than zero.';
	}
	if (form.categorySlugs.length === 0 && normalizeTags(form.tags).length === 0) {
		errors.scope = 'Pick at least one category or tag.';
	}
	if (form.periodKind === 'FIXED') {
		if (form.startDate === '') errors.startDate = 'A fixed goal needs a start date.';
		if (form.endDate !== '' && form.startDate !== '' && form.endDate < form.startDate) {
			errors.endDate = 'The end date cannot be before the start date.';
		}
	}
	return errors;
}

/** Build the mutation input; recurring sends no dates, fixed sends no cadence (the server rejects both). */
export function formToInput(form: GoalFormState, currency = 'EUR'): GoalInput {
	const base: GoalInput = {
		name: form.name.trim(),
		type: form.type,
		amount: formatScaled(tryParse(form.amount.trim()), 2),
		currency,
		categorySlugs: form.categorySlugs,
		tags: normalizeTags(form.tags),
		combine: form.combine,
		tagCombine: form.tagCombine,
		periodKind: form.periodKind
	};
	if (form.periodKind === 'RECURRING') return { ...base, cadence: form.cadence };
	return { ...base, startDate: form.startDate, endDate: form.endDate === '' ? null : form.endDate };
}
