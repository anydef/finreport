/**
 * Pure shaping for the curation-progress card. No Svelte, no stores — covered
 * by `curationView.test.ts`.
 *
 * There is no stored history of the uncategorised / needs-review counts, so
 * "progress" is relative to a **session baseline**: the count when this tab
 * first showed the card, held in `sessionStorage`. Every line says "since you
 * opened this page" and never claims a daily or all-time figure.
 *
 * Storage can be absent or throw (private window, blocked site data), so every
 * access is wrapped; a missing baseline means "no delta to show", never zero.
 */

export type CurationMetric = 'uncategorized' | 'needsReview';

export const CURATION_METRICS: readonly CurationMetric[] = ['uncategorized', 'needsReview'];

const SINCE = 'since you opened this page';

/** The slice of `Storage` this module needs; tests pass a fake. */
export type BaselineStorage = Pick<Storage, 'getItem' | 'setItem'>;

export function baselineKey(metric: CurationMetric): string {
	return `finreport.curation.baseline.${metric}`;
}

/** `sessionStorage` if the browser allows touching it at all, else `null`. */
function defaultStorage(): BaselineStorage | null {
	try {
		return typeof sessionStorage === 'undefined' ? null : sessionStorage;
	} catch {
		return null;
	}
}

/** The stored baseline, or `null` when absent, malformed or storage throws. */
export function readBaseline(
	metric: CurationMetric,
	storage: BaselineStorage | null = defaultStorage()
): number | null {
	try {
		const raw = storage?.getItem(baselineKey(metric));
		if (raw === null || raw === undefined || raw === '') return null;
		const n = Number(raw);
		return Number.isInteger(n) && n >= 0 ? n : null;
	} catch {
		return null;
	}
}

/** Returns whether the write stuck; a failure is harmless. */
export function writeBaseline(
	metric: CurationMetric,
	count: number,
	storage: BaselineStorage | null = defaultStorage()
): boolean {
	try {
		if (!storage) return false;
		storage.setItem(baselineKey(metric), String(count));
		return true;
	} catch {
		return false;
	}
}

/**
 * The baseline to compare against: the stored one, or — on the first view of
 * the session — `seed ?? current`, which is then stored. `null` only when
 * storage is unusable and there is no seed, so no delta is shown.
 *
 * `seed` exists for mock mode, so a reviewer can see a delta without
 * categorising anything; real mode never passes one.
 */
export function resolveBaseline(
	metric: CurationMetric,
	current: number,
	seed?: number,
	storage: BaselineStorage | null = defaultStorage()
): number | null {
	const stored = readBaseline(metric, storage);
	if (stored !== null) return stored;
	const initial = seed ?? current;
	const kept = writeBaseline(metric, initial, storage);
	// If storage refused the write, a seed is still honest to show for this
	// render; an unseeded `current` would only ever say "no change".
	return kept || seed !== undefined ? initial : null;
}

/**
 * The delta sentence, or `null` when there is nothing to say (no baseline, or
 * no change). A rise is stated plainly: an import can land mid-session.
 */
export function progressLine(current: number, baseline: number | null): string | null {
	if (baseline === null) return null;
	const diff = current - baseline;
	if (diff === 0) return null;
	const n = Math.abs(diff);
	return `${n} ${diff < 0 ? 'fewer' : 'more'} ${SINCE}`;
}

/** `undefined` (the backend cannot say yet) omits the line entirely. */
export function rulesLearnedLine(n: number | undefined): string | null {
	if (n === undefined) return null;
	if (n === 0)
		return 'No rules learned yet — categorise a repeated merchant and one is learned for you';
	if (n === 1) return '1 rule learned from your decisions';
	return `${n} rules learned from your decisions`;
}

export interface CurationRow {
	metric: CurationMetric;
	count: number;
	label: string;
	/** `null` when unchanged or when there is no baseline. */
	delta: string | null;
}

const LABELS: Record<CurationMetric, string> = {
	uncategorized: 'uncategorised',
	needsReview: 'need review'
};

export interface CurationInput {
	counts: Record<CurationMetric, number>;
	baselines: Record<CurationMetric, number | null>;
}

/**
 * The rows worth showing. A metric with a zero count and no movement is left
 * out (a clean queue is not worth a row of zeroes), but one that just fell to
 * zero stays, because that is the progress being reported.
 */
export function curationRows({ counts, baselines }: CurationInput): CurationRow[] {
	return CURATION_METRICS.map((metric) => ({
		metric,
		count: counts[metric],
		label: LABELS[metric],
		delta: progressLine(counts[metric], baselines[metric])
	})).filter((row) => row.count > 0 || row.delta !== null);
}
