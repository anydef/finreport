/**
 * Pure logic behind the grouped review queue: which view the URL asks for,
 * what a group's LLM proposal amounts to, and the filter/category input a
 * group-level action sends. Kept out of the components so it is unit-tested.
 */
import { reviewQueueFilter } from './bulkSelection';
import type { Category, HeldMerchantGroup, TransactionFilter } from './graphql/types';

export type ReviewView = 'groups' | 'flat';

/** `?view=flat` selects the flat list; anything else (or nothing) is the grouped default. */
export function parseView(param: string | null | undefined): ReviewView {
	return param === 'flat' ? 'flat' : 'groups';
}

/**
 * The review URL for a view, optionally with one group opened (`expand` is a
 * counterparty key). The grouped view is the default, so it carries no `view`.
 */
export function reviewHref(view: ReviewView, expand?: string | null): string {
	const params = new URLSearchParams();
	if (view === 'flat') params.set('view', 'flat');
	if (expand && view === 'groups') params.set('expand', expand);
	const qs = params.toString();
	return qs ? `?${qs}` : '?';
}

/**
 * The old `?counterpartyKey=` link meant "this merchant's held transactions";
 * that is now an expanded group. Returns the key to expand, from `expand` or
 * from that legacy parameter.
 */
export function expandedKey(params: URLSearchParams): string | null {
	return params.get('expand') || params.get('counterpartyKey') || null;
}

/** Only a group with a key can be addressed by a filter; the keyless bucket cannot. */
export function isAssignable(group: Pick<HeldMerchantGroup, 'counterpartyKey'>): boolean {
	return Boolean(group.counterpartyKey);
}

/** The filter that selects exactly this group's held transactions. */
export function groupFilter(group: Pick<HeldMerchantGroup, 'counterpartyKey'>): TransactionFilter {
	if (!group.counterpartyKey) throw new Error('A group without a merchant key has no filter.');
	return reviewQueueFilter(group.counterpartyKey);
}

export type Proposal =
	| { kind: 'none' }
	| { kind: 'unanimous'; path: string }
	| { kind: 'mixed'; path: string; votes: number; total: number };

/**
 * What the LLM proposed for a group. Unanimous means every held transaction
 * carries the same proposal (safe to accept in one click); mixed means the
 * transactions disagree or some have none, so the group's category is a decision.
 */
export function proposalOf(
	group: Pick<HeldMerchantGroup, 'proposedCategoryPath' | 'proposedCategoryVotes' | 'heldCount'>
): Proposal {
	const path = group.proposedCategoryPath;
	if (!path) return { kind: 'none' };
	if (group.proposedCategoryVotes >= group.heldCount) return { kind: 'unanimous', path };
	return { kind: 'mixed', path, votes: group.proposedCategoryVotes, total: group.heldCount };
}

/** Input for `ensureCategory` from a proposed dotted path (`housing.coworking`). */
export function proposedCategoryInput(path: string): {
	slug: string;
	name: string;
	kind: 'EXPENSE';
	parentSlug?: string;
} {
	const segments = path.split('.');
	const name = segments[segments.length - 1]
		.split('_')
		.map((w) => w.charAt(0).toUpperCase() + w.slice(1))
		.join(' ');
	const parentSlug = segments.length > 1 ? segments.slice(0, -1).join('.') : undefined;
	return { slug: path, name, kind: 'EXPENSE', parentSlug };
}

/** Whether accepting `path` must create the category first. */
export function needsCreating(path: string, categories: Pick<Category, 'slug'>[]): boolean {
	return !categories.some((c) => c.slug === path);
}

export function reasonLabels(reasons: HeldMerchantGroup['reviewReasons']): string[] {
	return reasons.map((r) => (r === 'NEW_CATEGORY' ? 'New category' : 'Ambiguous'));
}
