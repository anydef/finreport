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

export interface RuleConditions {
	counterpartyKey?: string;
	counterpartyIban?: string;
	descriptionRegex?: string;
	descriptionContains?: string;
	direction?: 'INCOME' | 'SPENDING';
	amountMin?: string;
	amountMax?: string;
	accountIds?: string[];
}

function asConditions(conditions: Json): RuleConditions {
	return (conditions ?? {}) as RuleConditions;
}

/**
 * Specificity score per §2.7: `counterpartyIban`/`counterpartyKey` weigh 2
 * each, a bounded amount range (`amountMin` *and* `amountMax` both present)
 * weighs 1, every other present condition weighs 1.
 */
export function specificityScore(conditions: Json): number {
	const c = asConditions(conditions);
	let score = 0;
	if (c.counterpartyIban) score += 2;
	if (c.counterpartyKey) score += 2;
	if (c.amountMin !== undefined && c.amountMax !== undefined) score += 1;
	if (c.descriptionRegex) score += 1;
	if (c.descriptionContains) score += 1;
	if (c.direction) score += 1;
	if (c.accountIds && c.accountIds.length > 0) score += 1;
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
	const parts: string[] = [];
	if (c.counterpartyKey) parts.push(`counterparty = "${c.counterpartyKey}"`);
	if (c.counterpartyIban) parts.push(`IBAN = ${c.counterpartyIban}`);
	if (c.descriptionContains) parts.push(`description contains "${c.descriptionContains}"`);
	if (c.descriptionRegex) parts.push(`description matches /${c.descriptionRegex}/`);
	if (c.direction) parts.push(`direction = ${c.direction}`);
	if (c.amountMin !== undefined || c.amountMax !== undefined) {
		parts.push(`amount ${c.amountMin ?? '…'} – ${c.amountMax ?? '…'}`);
	}
	if (c.accountIds && c.accountIds.length > 0) {
		parts.push(`account in [${c.accountIds.length}]`);
	}
	return parts;
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
