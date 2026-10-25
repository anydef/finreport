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
export function reviewHref(
	view: ReviewView,
	expand?: string | null,
	expandIban?: string | null
): string {
	const params = new URLSearchParams();
	if (view === 'flat') params.set('view', 'flat');
	if (expand && view === 'groups') {
		params.set('expand', expand);
		// A split key needs the IBAN to name one of its groups: `expandIban=` with
		// no value is the key's no-IBAN group, an absent parameter is "any".
		if (expandIban !== undefined) params.set('expandIban', expandIban ?? '');
	}
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

/**
 * Which IBAN of the expanded key is open: `undefined` = the parameter is
 * absent (a link that names only the key), `null` = the key's no-IBAN group,
 * a string = that IBAN's group.
 */
export function expandedIban(params: URLSearchParams): string | null | undefined {
	if (!params.has('expandIban')) return undefined;
	return params.get('expandIban') || null;
}

/** The identity of a group: its key and IBAN (`ibanDisambiguated` says whether the IBAN matters). */
export type GroupRef = Pick<HeldMerchantGroup, 'counterpartyKey'> &
	Partial<Pick<HeldMerchantGroup, 'counterpartyIban' | 'ibanDisambiguated'>>;

/** Whether `group` is the one the URL has open. */
export function isExpanded(
	group: GroupRef,
	expand: string | null,
	iban: string | null | undefined
): boolean {
	if (group.counterpartyKey === null || group.counterpartyKey !== expand) return false;
	return iban === undefined || iban === (group.counterpartyIban ?? null);
}

/** A stable `{#each}` key: a split key has several groups, so the key alone repeats. */
export function groupId(group: GroupRef): string {
	return `${group.counterpartyKey ?? '\u0000ungrouped'}\u0000${group.counterpartyIban ?? ''}`;
}

/**
 * Only a group with a key can be addressed by a filter. The keyless buckets
 * stay unassignable even when they carry an IBAN: no filter says "key is
 * empty" (`hasCounterparty` is about the name), so `counterpartyIbans` alone
 * would also select keyed rows that share that IBAN and a bulk assign would
 * miscategorise them.
 */
export function isAssignable(group: Pick<HeldMerchantGroup, 'counterpartyKey'>): boolean {
	return Boolean(group.counterpartyKey);
}

/**
 * The filter that selects exactly this group's held transactions, mirroring
 * `counterpartyHref`: the key alone when the key is not split, plus the IBAN
 * when it is, or plus `hasCounterpartyIban: false` for the split key's
 * no-IBAN group. Omitting the IBAN of a split group would also touch its
 * siblings' transactions.
 */
export function groupFilter(group: GroupRef): TransactionFilter {
	if (!group.counterpartyKey) throw new Error('A group without a merchant key has no filter.');
	const filter = reviewQueueFilter(group.counterpartyKey);
	if (!group.ibanDisambiguated) return filter;
	return group.counterpartyIban
		? { ...filter, counterpartyIbans: [group.counterpartyIban] }
		: { ...filter, hasCounterpartyIban: false };
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
