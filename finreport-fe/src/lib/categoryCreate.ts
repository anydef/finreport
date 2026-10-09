/**
 * Creating a category from the transaction editor. The slug rule (a child's
 * slug is its parent's plus exactly one segment) is not re-derived here: it
 * lives in `categorySlug.ts` and this module only assembles the
 * `CategoryInput` around it.
 */
import { composeSlug, MAX_SLUG_DEPTH, validateLeaf } from './categorySlug';
import type { Category, CategoryInput, CategoryKind } from './graphql/types';

export type CategoryPlan =
	| { ok: true; input: CategoryInput; slug: string }
	| { ok: false; problem: string };

/** A leaf suggestion from the display name: `Board games!` -> `board_games`. */
export function leafFromName(name: string): string {
	return name
		.normalize('NFKD')
		.toLowerCase()
		.replace(/[^a-z0-9]+/g, '_')
		.replace(/^_+|_+$/g, '');
}

/** Categories a new one can nest under: active and not already at the depth limit. */
export function parentCandidates(categories: Category[]): Category[] {
	return categories
		.filter((c) => !c.archived && c.slug.split('.').length < MAX_SLUG_DEPTH)
		.sort((a, b) => a.slug.localeCompare(b.slug));
}

/** A child inherits its parent's kind; a top-level category defaults to an expense. */
export function defaultKind(parentSlug: string | null, categories: Category[]): CategoryKind {
	return categories.find((c) => c.slug === parentSlug)?.kind ?? 'EXPENSE';
}

export function planCategory(args: {
	parentSlug: string | null;
	leaf: string;
	name: string;
	kind: CategoryKind;
}): CategoryPlan {
	const problem = validateLeaf(args.parentSlug, args.leaf);
	if (problem) return { ok: false, problem };
	const name = args.name.trim();
	if (name === '') return { ok: false, problem: 'Enter a name.' };
	const slug = composeSlug(args.parentSlug, args.leaf);
	return {
		ok: true,
		slug,
		input: { slug, name, kind: args.kind, parentSlug: args.parentSlug }
	};
}

/** Add `created` to the list, replacing an entry with the same slug (ensureCategory may return an existing one). */
export function withCategory(categories: Category[], created: Category): Category[] {
	return [...categories.filter((c) => c.slug !== created.slug), created];
}
