/**
 * Client-side tag normalization (iteration 3 §4/§5), previewed before
 * `setTransactionTags` round-trips and re-normalizes server-side — this
 * mirrors the server's normalization table case-for-case so the preview
 * never surprises the user:
 *
 * NFKC, lowercase, trim, collapse internal whitespace/`_` to `-`, strip
 * anything outside `[a-z0-9-]`, collapse repeated `-`, trim leading/
 * trailing `-`. Empty after normalization is dropped; length must be
 * `1..=32`; duplicates collapse silently.
 */

/** Hard cap from `APP_max_tags_per_transaction`'s spec default (§2.4). */
export const MAX_TAGS_PER_TRANSACTION = 10;

const MIN_TAG_LENGTH = 1;
const MAX_TAG_LENGTH = 32;

/** Normalize a single raw tag string; `''` means "drop it". */
export function normalizeTag(raw: string): string {
	const nfkc = raw.normalize('NFKC').toLowerCase().trim();
	const dashed = nfkc.replace(/[\s_]+/g, '-');
	const stripped = dashed.replace(/[^a-z0-9-]/g, '');
	const collapsed = stripped.replace(/-+/g, '-').replace(/^-+|-+$/g, '');
	if (collapsed.length < MIN_TAG_LENGTH || collapsed.length > MAX_TAG_LENGTH) return '';
	return collapsed;
}

/**
 * Normalize a whole set: dedupe, drop empties, sort, and cap at
 * `MAX_TAGS_PER_TRANSACTION` (extras silently dropped — the server would
 * instead reject with `TOO_MANY_TAGS`; the client-side cap just keeps the
 * editor from building a set guaranteed to fail).
 */
export function normalizeTags(raw: string[]): string[] {
	const seen = new Set<string>();
	for (const tag of raw) {
		const normalized = normalizeTag(tag);
		if (normalized) seen.add(normalized);
	}
	return [...seen].sort().slice(0, MAX_TAGS_PER_TRANSACTION);
}

/** Add one raw tag to an existing (already-normalized) set, re-normalizing the result. */
export function addTag(existing: string[], raw: string): string[] {
	return normalizeTags([...existing, raw]);
}

/** Remove one tag from an existing set. */
export function removeTag(existing: string[], tag: string): string[] {
	return existing.filter((t) => t !== tag);
}

/** Whether a raw tag is non-empty after normalization — gate for "can be added". */
export function isValidTag(raw: string): boolean {
	return normalizeTag(raw) !== '';
}
