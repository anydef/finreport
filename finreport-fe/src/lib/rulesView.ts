/**
 * Pure, non-Svelte helpers behind `/admin/rules` and `/admin/categories`
 * (§6, §10 WP6): rule sort/specificity, badge/grouping logic for rule
 * display, match-condition summaries, and category-tree shaping. Covered by
 * `rulesView.test.ts` — no component-render test setup exists in this
 * project (see `finreport-fe/CLAUDE.md`).
 */

import type { Category, Json, Rule, RuleState } from './graphql/types';

// ---------------------------------------------------------------------------
// Rule ordering (§2.7: "most specific wins")
// ---------------------------------------------------------------------------

/**
 * The `conditions` JSON exactly as the backend stores and validates it:
 * snake_case keys (`validate_conditions` rejects anything else), decimals as
 * strings (a bare JSON number is tolerated on read). This is the one
 * conditions type for the whole frontend: scoring, summaries and the form
 * all read and write these keys.
 */
export interface RuleConditions {
	counterparty_key?: string;
	counterparty_iban?: string;
	description_regex?: string;
	description_contains?: string;
	direction?: 'INCOME' | 'SPENDING';
	amount_min?: string | number;
	amount_max?: string | number;
	account_ids?: string[];
}

function asConditions(conditions: Json): RuleConditions {
	return (conditions ?? {}) as RuleConditions;
}

/**
 * Specificity score per §2.7: `counterparty_iban`/`counterparty_key` weigh 2
 * each, a bounded amount range (`amount_min` *and* `amount_max` both
 * present) weighs 1, every other present condition weighs 1.
 */
export function specificityScore(conditions: Json): number {
	const c = asConditions(conditions);
	const present = (v: unknown) => v !== undefined && v !== null && v !== '';
	let score = 0;
	if (c.counterparty_iban) score += 2;
	if (c.counterparty_key) score += 2;
	if (present(c.amount_min) && present(c.amount_max)) score += 1;
	if (c.description_regex) score += 1;
	if (c.description_contains) score += 1;
	if (c.direction) score += 1;
	if (c.account_ids && c.account_ids.length > 0) score += 1;
	return score;
}

/**
 * Sort rules the same way the labeler picks a match: `priority` descending,
 * then specificity descending, then `id` ascending as the deterministic
 * tie-breaker (§2.7) — never row order.
 */
export function sortRulesBySpecificity(rules: Rule[]): Rule[] {
	return [...rules].sort((a, b) => {
		if (a.priority !== b.priority) return b.priority - a.priority;
		const specDiff = specificityScore(b.conditions) - specificityScore(a.conditions);
		if (specDiff !== 0) return specDiff;
		return a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
	});
}

/** One human-readable line per present condition, AND-ed together (§2.7). */
export function summarizeConditions(conditions: Json): string[] {
	const c = asConditions(conditions);
	const present = (v: unknown) => v !== undefined && v !== null && v !== '';
	const parts: string[] = [];
	if (c.counterparty_key) parts.push(`counterparty = "${c.counterparty_key}"`);
	if (c.counterparty_iban) parts.push(`IBAN = ${c.counterparty_iban}`);
	if (c.description_contains) parts.push(`description contains "${c.description_contains}"`);
	if (c.description_regex) parts.push(`description matches /${c.description_regex}/`);
	if (c.direction) parts.push(`direction = ${c.direction}`);
	if (present(c.amount_min) || present(c.amount_max)) {
		parts.push(
			`amount ${present(c.amount_min) ? c.amount_min : '…'} – ${present(c.amount_max) ? c.amount_max : '…'}`
		);
	}
	if (c.account_ids && c.account_ids.length > 0) {
		parts.push(`account in [${c.account_ids.length}]`);
	}
	return parts;
}

/** The condition keys `RuleForm` has an input for; every other stored key is carried through an edit untouched. */
export type EditableConditions = {
	counterparty_key: string;
	counterparty_iban: string;
	description_contains: string;
	description_regex: string;
	direction: '' | 'INCOME' | 'SPENDING';
	amount_min: string;
	amount_max: string;
};

/**
 * The `conditions` to submit for an edit: the rule's stored conditions with
 * each form-editable key set (trimmed, non-empty) or removed (empty). Keys
 * the form has no field for (`account_ids`) survive unchanged, so editing a
 * rule never silently drops them. Keys are the backend's snake_case.
 */
export function mergeConditions(
	existing: Json,
	edited: EditableConditions
): Record<string, unknown> {
	const merged: Record<string, unknown> = { ...(existing as Record<string, unknown> | null) };
	for (const [key, raw] of Object.entries(edited)) {
		const value = raw.trim();
		if (value) merged[key] = value;
		else delete merged[key];
	}
	return merged;
}

/** The "matches N transactions" cell text; the zero state is spelled out, not a bare 0. */
/** The merchant key a rule is about (`conditions.counterparty_key`), or null for a rule that is not merchant-specific. */
export function merchantKeyOf(rule: Pick<Rule, 'conditions'>): string | null {
	const key = asConditions(rule.conditions).counterparty_key;
	return typeof key === 'string' && key.trim() !== '' ? key.trim() : null;
}

/** What the user is committing to for a merchant, in plain words (shown in the confirm dialog and the exempt list). */
export function describeExemptionImpact(transactionCount: number): string {
	if (transactionCount === 0) return 'No transactions in your accounts yet.';
	const noun = transactionCount === 1 ? 'transaction' : 'transactions';
	return `${transactionCount} ${noun} in your accounts to label or split by hand.`;
}

export function describeReach(count: number): string {
	if (count === 0) return 'matches nothing';
	return `${count} transaction${count === 1 ? '' : 's'}`;
}

// ---------------------------------------------------------------------------
// Badges / grouping (§6, §2.8)
// ---------------------------------------------------------------------------

/** A rule shows the "auto-approved" badge iff it was learned and published above the threshold (§2.8). */
export function isAutoApproved(rule: Rule): boolean {
	return rule.origin === 'LEARNED' && rule.autoApproved;
}

export interface RuleGroups {
	active: Rule[];
	inReview: Rule[];
	revoked: Rule[];
	rejected: Rule[];
}

const STATE_KEYS: Record<RuleState, keyof RuleGroups> = {
	ACTIVE: 'active',
	IN_REVIEW: 'inReview',
	REVOKED: 'revoked',
	REJECTED: 'rejected'
};

/** Group rules by state, each group internally ordered by §2.7 specificity. */
export function groupRulesByState(rules: Rule[]): RuleGroups {
	const groups: RuleGroups = { active: [], inReview: [], revoked: [], rejected: [] };
	for (const rule of rules) {
		groups[STATE_KEYS[rule.state]].push(rule);
	}
	for (const key of Object.keys(groups) as (keyof RuleGroups)[]) {
		groups[key] = sortRulesBySpecificity(groups[key]);
	}
	return groups;
}

/** Newest-first, for the "recently auto-approved" section (§6). */
export function sortByRecentlyCreated(rules: Rule[]): Rule[] {
	return [...rules].sort((a, b) => b.createdAt.localeCompare(a.createdAt));
}

// ---------------------------------------------------------------------------
// Category tree (§5, §6: "the tree, create/rename/archive, kind per node,
// depth-3 nodes refusing children in the UI as well as the API")
// ---------------------------------------------------------------------------

export const MAX_CATEGORY_DEPTH = 3;

export interface CategoryNode extends Category {
	children: CategoryNode[];
}

/** Build a tree from the flat `categories` query result (depth is server-assigned, 1-based). */
export function buildCategoryTree(categories: Category[]): CategoryNode[] {
	const byId = new Map<string, CategoryNode>();
	for (const category of categories) {
		byId.set(category.id, { ...category, children: [] });
	}
	const roots: CategoryNode[] = [];
	for (const category of categories) {
		const node = byId.get(category.id)!;
		const parent = category.parentId ? byId.get(category.parentId) : undefined;
		if (parent) {
			parent.children.push(node);
		} else {
			roots.push(node);
		}
	}
	const byName = (a: CategoryNode, b: CategoryNode) => a.name.localeCompare(b.name);
	const sortTree = (nodes: CategoryNode[]) => {
		nodes.sort(byName);
		for (const node of nodes) sortTree(node.children);
	};
	sortTree(roots);
	return roots;
}

/** A node may take a child only while it hasn't reached the depth-3 cap (§5). */
export function canHaveChildren(node: Pick<Category, 'depth'>): boolean {
	return node.depth < MAX_CATEGORY_DEPTH;
}

/** "Groceries" under "Food" → "Food / Groceries", for flat `<select>`s (RuleForm, create-category parent picker). */
export function flattenCategoryTreeWithPath(
	nodes: CategoryNode[],
	parentPath: string[] = []
): { category: Category; path: string; depth: number }[] {
	const out: { category: Category; path: string; depth: number }[] = [];
	for (const node of nodes) {
		const path = [...parentPath, node.name];
		out.push({ category: node, path: path.join(' / '), depth: node.depth });
		out.push(...flattenCategoryTreeWithPath(node.children, path));
	}
	return out;
}
