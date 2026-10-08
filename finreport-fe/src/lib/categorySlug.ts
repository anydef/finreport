/**
 * Category slugs encode the tree: `leisure.hobbies.games` is a child of
 * `leisure.hobbies`. The backend requires a child's slug to be its parent's
 * slug plus exactly one segment, so the admin dialog asks only for the leaf
 * and composes the full slug here.
 */

export const MAX_SLUG_DEPTH = 3;

/** Shown next to the input: what a single segment may contain. */
export const SEGMENT_HINT = 'Lowercase letters, digits and underscores only.';

const SEGMENT = /^[a-z0-9_]+$/;

/**
 * Normalises what the user typed into a bare leaf. Surrounding whitespace is
 * dropped. If they pasted the full path including the parent (`leisure.gym`
 * under `leisure`) the parent prefix is stripped, since that is unambiguous;
 * any other dotted input is left as is so `validateLeaf` rejects it rather
 * than guessing which segment was meant.
 */
export function leafFromInput(parentSlug: string | null, raw: string): string {
	const trimmed = raw.trim();
	if (parentSlug && trimmed.startsWith(`${parentSlug}.`)) {
		return trimmed.slice(parentSlug.length + 1);
	}
	return trimmed;
}

/** The slug the backend will receive for this leaf. */
export function composeSlug(parentSlug: string | null, raw: string): string {
	const leaf = leafFromInput(parentSlug, raw);
	return parentSlug ? `${parentSlug}.${leaf}` : leaf;
}

/** `null` when the leaf is acceptable, otherwise a message for the user. */
export function validateLeaf(parentSlug: string | null, raw: string): string | null {
	if (parentSlug && parentSlug.split('.').length >= MAX_SLUG_DEPTH) {
		return `"${parentSlug}" is already ${MAX_SLUG_DEPTH} levels deep; categories nest at most ${MAX_SLUG_DEPTH} levels.`;
	}
	const leaf = leafFromInput(parentSlug, raw);
	if (leaf === '') return 'Enter a slug.';
	if (leaf.includes('.')) {
		return parentSlug
			? `Enter one segment only; "${parentSlug}." is added for you.`
			: 'A top-level slug is a single segment with no dots; create subcategories from their parent.';
	}
	if (!SEGMENT.test(leaf)) return SEGMENT_HINT;
	return null;
}
