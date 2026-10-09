/**
 * Autocomplete for the tag editor. Candidates come from `Query.tags` (every
 * tag in the book with its usage count), not from the rows on screen, so a
 * tag used only on another page is still offered. Everything suggested is in
 * normalised form (`tags.ts`), so accepting a suggestion saves exactly what
 * was shown.
 */
import { MAX_TAGS_PER_TRANSACTION, normalizeTag } from './tags';
import type { TagCount } from './graphql/types';

export interface TagSuggestion {
	tag: string;
	/** Transactions already carrying the tag; `null` for a tag that does not exist yet. */
	count: number | null;
	isNew: boolean;
}

export const DEFAULT_SUGGESTION_LIMIT = 8;

function byCountThenName(a: TagCount, b: TagCount): number {
	return b.transactionCount - a.transactionCount || a.tag.localeCompare(b.tag);
}

/**
 * Suggestions for what has been typed so far.
 *
 * - An empty draft lists the most-used tags not already on the transaction.
 * - Otherwise prefix matches rank before substring matches, each by usage
 *   count (then alphabetically), and tags already on the transaction are left out.
 * - If the normalised draft is not an existing tag, a trailing "new tag"
 *   entry offers it, so free-form tags stay one keypress away.
 * - A draft that normalises to nothing (`"!!"`) yields no suggestions.
 */
export function suggestTags(
	known: TagCount[],
	draft: string,
	current: string[],
	limit: number = DEFAULT_SUGGESTION_LIMIT
): TagSuggestion[] {
	const taken = new Set(current);
	const pool = known.filter((t) => !taken.has(t.tag));

	if (draft.trim() === '') {
		return [...pool]
			.sort(byCountThenName)
			.slice(0, limit)
			.map((t) => ({ tag: t.tag, count: t.transactionCount, isNew: false }));
	}

	const needle = normalizeTag(draft);
	if (needle === '') return [];

	const prefix = pool.filter((t) => t.tag.startsWith(needle)).sort(byCountThenName);
	const inner = pool
		.filter((t) => !t.tag.startsWith(needle) && t.tag.includes(needle))
		.sort(byCountThenName);
	const out: TagSuggestion[] = [...prefix, ...inner]
		.slice(0, limit)
		.map((t) => ({ tag: t.tag, count: t.transactionCount, isNew: false }));

	const exists = known.some((t) => t.tag === needle) || taken.has(needle);
	if (!exists && current.length < MAX_TAGS_PER_TRANSACTION) {
		out.push({ tag: needle, count: null, isNew: true });
	}
	return out;
}

/** Next highlighted index for an arrow key; `-1` means "nothing highlighted" (Enter adds the typed text). */
export function moveHighlight(active: number, key: 'ArrowDown' | 'ArrowUp', size: number): number {
	if (size === 0) return -1;
	if (key === 'ArrowDown') return active + 1 >= size ? 0 : active + 1;
	return active <= 0 ? size - 1 : active - 1;
}
