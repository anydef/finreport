/**
 * The transaction filter panel's logic, kept out of the Svelte component so
 * it is testable: reading the filter out of search params, writing it back,
 * deciding what counts as "active", the tri-state flag mapping, and layering a
 * chart selection over the panel on the dashboard.
 *
 * The panel owns these search params on every page that shows it, so a
 * filtered view is linkable: `accountIds`, `categorySlugs`, `tags`, `search`,
 * `amountMin`, `amountMax`, `recurring`, `transfer`, `needsReview`,
 * `uncategorized`, `reimbursements` (`exclude` | `only`; absent = include).
 */
import { expandSelectedSlugs } from '$lib/categoryTree';
import type { Category, Decimal, TransactionFilter } from '$lib/graphql/types';
import {
	reimbursementChoiceFromParam,
	reimbursementFilterValue,
	type ReimbursementChoice
} from '$lib/reimbursement';

/** A flag: `true` = only matching, `false` = only non-matching, `undefined` = don't care. */
export type Tri = boolean | undefined;
/** The same flag as a form value; a `<select>` or radio cannot hold `undefined`. */
export type TriValue = 'any' | 'yes' | 'no';

export const FLAG_KEYS = ['recurring', 'transfer', 'needsReview', 'uncategorized'] as const;
export type FlagKey = (typeof FLAG_KEYS)[number];

export interface PanelFilters {
	accountIds: string[];
	categorySlugs: string[];
	tags: string[];
	/** Magnitude bounds, normalised; `undefined` = unbounded. */
	amountMin: string | undefined;
	amountMax: string | undefined;
	search: string;
	recurring: Tri;
	transfer: Tri;
	needsReview: Tri;
	uncategorized: Tri;
	/**
	 * Whether the offsetting side of reimbursement links counts. Absent means
	 * included, so a view nobody changed shows the money that actually moved;
	 * `exclude` is how an income view leaves reimbursements out (genuine income
	 * is never on that side), `only` lists just the reimbursements.
	 */
	reimbursements: ReimbursementChoice;
}

/** Every search param the panel owns; everything else passes through untouched. */
export const PANEL_PARAMS = [
	'accountIds',
	'categorySlugs',
	'tags',
	'search',
	'amountMin',
	'amountMax',
	'reimbursements',
	...FLAG_KEYS
] as const;

/** The panel with nothing set (a fresh object each call, so it is safe to mutate). */
export function clearedFilters(): PanelFilters {
	return {
		accountIds: [],
		categorySlugs: [],
		tags: [],
		amountMin: undefined,
		amountMax: undefined,
		search: '',
		recurring: undefined,
		transfer: undefined,
		needsReview: undefined,
		uncategorized: undefined,
		reimbursements: undefined
	};
}

export function triToValue(tri: Tri): TriValue {
	return tri === true ? 'yes' : tri === false ? 'no' : 'any';
}

export function triFromValue(value: string): Tri {
	return value === 'yes' ? true : value === 'no' ? false : undefined;
}

function triFromParam(raw: string | null): Tri {
	return raw === 'true' ? true : raw === 'false' ? false : undefined;
}

function list(raw: string | null): string[] {
	return raw?.split(',').filter(Boolean) ?? [];
}

/**
 * Normalise a typed amount bound to a plain non-negative decimal string, or
 * `undefined` when empty or not a usable number. Bounds are magnitudes, so a
 * leading minus is dropped rather than rejected ("-100" and "100" mean the same).
 */
export function normalizeAmount(raw: string | null | undefined): string | undefined {
	if (raw === null || raw === undefined) return undefined;
	const cleaned = raw.trim().replace(',', '.').replace(/^[-+]/, '');
	if (!/^(\d+(\.\d*)?|\.\d+)$/.test(cleaned)) return undefined;
	return cleaned.startsWith('.') ? `0${cleaned}` : cleaned.replace(/\.$/, '');
}

/** Read the panel out of search params (also accepts the legacy single `accountId`). */
export function parsePanelFilters(params: URLSearchParams): PanelFilters {
	const accountIds = list(params.get('accountIds'));
	const legacyAccount = params.get('accountId');
	if (!accountIds.length && legacyAccount) accountIds.push(legacyAccount);
	return {
		accountIds,
		categorySlugs: list(params.get('categorySlugs')),
		tags: list(params.get('tags')),
		amountMin: normalizeAmount(params.get('amountMin')),
		amountMax: normalizeAmount(params.get('amountMax')),
		search: params.get('search')?.trim() ?? '',
		recurring: triFromParam(params.get('recurring')),
		transfer: triFromParam(params.get('transfer')),
		needsReview: triFromParam(params.get('needsReview')),
		uncategorized: triFromParam(params.get('uncategorized')),
		reimbursements: reimbursementChoiceFromParam(params.get('reimbursements'))
	};
}

/**
 * Write the panel into a copy of `params`. Every panel key is cleared first,
 * so a removed filter really disappears, and params the panel does not own
 * (period, paging, ...) pass through.
 */
export function writePanelFilters(
	filters: PanelFilters,
	params: URLSearchParams = new URLSearchParams()
): URLSearchParams {
	const next = new URLSearchParams(params);
	for (const key of PANEL_PARAMS) next.delete(key);
	next.delete('accountId');
	if (filters.accountIds.length) next.set('accountIds', filters.accountIds.join(','));
	if (filters.categorySlugs.length) next.set('categorySlugs', filters.categorySlugs.join(','));
	if (filters.tags.length) next.set('tags', filters.tags.join(','));
	if (filters.search.trim()) next.set('search', filters.search.trim());
	if (filters.amountMin !== undefined) next.set('amountMin', filters.amountMin);
	if (filters.amountMax !== undefined) next.set('amountMax', filters.amountMax);
	if (filters.reimbursements) next.set('reimbursements', filters.reimbursements);
	for (const key of FLAG_KEYS) {
		const tri = filters[key];
		if (tri !== undefined) next.set(key, String(tri));
	}
	return next;
}

/**
 * The GraphQL filter fields for a panel. `categorySlugs` are expanded to
 * descendants here (the backend ORs the slugs it is given). Only set fields
 * are returned, so `{}` means "no restriction".
 */
export function toTransactionFilter(
	filters: PanelFilters,
	categories: Category[]
): Partial<TransactionFilter> {
	const out: Partial<TransactionFilter> = {};
	if (filters.accountIds.length) out.accountIds = filters.accountIds;
	if (filters.categorySlugs.length)
		out.categorySlugs = expandSelectedSlugs(filters.categorySlugs, categories);
	if (filters.tags.length) out.tags = filters.tags;
	if (filters.search) out.search = filters.search;
	if (filters.amountMin !== undefined) out.amountMin = filters.amountMin;
	if (filters.amountMax !== undefined) out.amountMax = filters.amountMax;
	const reimbursements = reimbursementFilterValue(filters.reimbursements);
	if (reimbursements) out.reimbursements = reimbursements;
	for (const key of FLAG_KEYS) {
		if (filters[key] !== undefined) out[key] = filters[key];
	}
	return out;
}

/** An amount range whose bounds contradict each other matches nothing. */
export function isAmountRangeInverted(filters: PanelFilters): boolean {
	return (
		filters.amountMin !== undefined &&
		filters.amountMax !== undefined &&
		Number(filters.amountMin) > Number(filters.amountMax)
	);
}

/** Inclusive magnitude test; the mock backend uses it to honour `amountMin`/`amountMax`. */
export function amountWithinRange(
	amount: Decimal,
	min: Decimal | null | undefined,
	max: Decimal | null | undefined
): boolean {
	const magnitude = Math.abs(Number(amount));
	if (min !== undefined && min !== null && magnitude < Number(min)) return false;
	if (max !== undefined && max !== null && magnitude > Number(max)) return false;
	return true;
}

export interface ActiveFilter {
	/** Stable id; also what `removeFilter` takes. */
	id: string;
	label: string;
}

export interface Labels {
	accounts: { id: string; label: string }[];
	categories: Category[];
}

const FLAG_LABELS: Record<FlagKey, [yes: string, no: string]> = {
	recurring: ['Recurring', 'Not recurring'],
	transfer: ['Transfer', 'Not a transfer'],
	needsReview: ['Needs review', 'Not needing review'],
	uncategorized: ['Uncategorized', 'Categorized']
};

export function describeAmountRange(filters: PanelFilters): string | undefined {
	const { amountMin: min, amountMax: max } = filters;
	if (min !== undefined && max !== undefined) return `Amount ${min} to ${max}`;
	if (min !== undefined) return `Amount at least ${min}`;
	if (max !== undefined) return `Amount at most ${max}`;
	return undefined;
}

/** The filters currently narrowing the result, one chip each (one per account/category/tag). */
export function activeFilters(filters: PanelFilters, labels: Labels): ActiveFilter[] {
	const out: ActiveFilter[] = [];
	for (const id of filters.accountIds) {
		const name = labels.accounts.find((a) => a.id === id)?.label ?? id;
		out.push({ id: `account:${id}`, label: `Account: ${name}` });
	}
	for (const slug of filters.categorySlugs) {
		const name = labels.categories.find((c) => c.slug === slug)?.name ?? slug;
		out.push({ id: `category:${slug}`, label: `Category: ${name}` });
	}
	for (const tag of filters.tags) out.push({ id: `tag:${tag}`, label: `Tag: ${tag}` });
	const range = describeAmountRange(filters);
	if (range) out.push({ id: 'amount', label: range });
	if (filters.search) out.push({ id: 'search', label: `Search: "${filters.search}"` });
	if (filters.reimbursements) {
		out.push({
			id: 'reimbursements',
			label: filters.reimbursements === 'exclude' ? 'Reimbursements left out' : 'Reimbursements only'
		});
	}
	for (const key of FLAG_KEYS) {
		const tri = filters[key];
		if (tri !== undefined) out.push({ id: `flag:${key}`, label: FLAG_LABELS[key][tri ? 0 : 1] });
	}
	return out;
}

export function hasActiveFilters(filters: PanelFilters): boolean {
	return activeFilters(filters, { accounts: [], categories: [] }).length > 0;
}

/** Drop the filter a chip stands for. */
export function removeFilter(filters: PanelFilters, id: string): PanelFilters {
	const [kind, ...rest] = id.split(':');
	const value = rest.join(':');
	switch (kind) {
		case 'account':
			return { ...filters, accountIds: filters.accountIds.filter((a) => a !== value) };
		case 'category':
			return { ...filters, categorySlugs: filters.categorySlugs.filter((c) => c !== value) };
		case 'tag':
			return { ...filters, tags: filters.tags.filter((t) => t !== value) };
		case 'amount':
			return { ...filters, amountMin: undefined, amountMax: undefined };
		case 'search':
			return { ...filters, search: '' };
		case 'reimbursements':
			return { ...filters, reimbursements: undefined };
		case 'flag':
			return FLAG_KEYS.includes(value as FlagKey) ? { ...filters, [value]: undefined } : filters;
		default:
			return filters;
	}
}

/**
 * Layer a chart selection (a Sankey/breakdown click) over the panel's filter
 * for the transaction list. On a dimension both set, the selection wins: it
 * was picked from a chart the panel already narrowed, so it is a subset, and
 * overriding is simpler and more predictable than intersecting id lists.
 */
export function layerSelection(
	panel: Partial<TransactionFilter>,
	selection: Partial<TransactionFilter>
): Partial<TransactionFilter> {
	const out: Record<string, unknown> = { ...panel };
	for (const [key, value] of Object.entries(selection)) {
		if (value !== undefined && value !== null) out[key] = value;
	}
	// A selected "Uncategorized" node contradicts category slugs from the panel, and vice versa.
	if (selection.uncategorized) delete out.categorySlugs;
	if (selection.categorySlugs?.length || selection.categorySlugsExact?.length) {
		delete out.uncategorized;
	}
	return out as Partial<TransactionFilter>;
}
