/**
 * Pure tree-shaping helpers over the flat `Category[]` the `categories` query
 * returns (iteration 2, §5/§6 WP5). No Svelte, no I/O — covered by
 * `categoryTree.test.ts`. Shared by `Tree.svelte`/`CategoryPicker.svelte`/
 * `CategoryFilter.svelte` and reused read-only by WP6's admin pages.
 */

import type { Category } from './graphql/types';

export interface CategoryTreeNode extends Category {
	children: CategoryTreeNode[];
}

/**
 * Build a forest of (at most 3-level deep, §5 `createCategory`) nodes from
 * the flat list the `categories` query returns, ordered by `name` at each
 * level. Archived categories are kept (callers filter them out explicitly,
 * e.g. for pickers) so a node's ancestors are never silently missing.
 */
export function buildCategoryTree(categories: Category[]): CategoryTreeNode[] {
	const nodes = new Map<string, CategoryTreeNode>();
	for (const category of categories) {
		nodes.set(category.id, { ...category, children: [] });
	}
	const roots: CategoryTreeNode[] = [];
	for (const category of categories) {
		const node = nodes.get(category.id)!;
		const parent = category.parentId ? nodes.get(category.parentId) : undefined;
		if (parent) parent.children.push(node);
		else roots.push(node);
	}
	const byName = (a: CategoryTreeNode, b: CategoryTreeNode) => a.name.localeCompare(b.name);
	const sortRec = (list: CategoryTreeNode[]) => {
		list.sort(byName);
		for (const n of list) sortRec(n.children);
	};
	sortRec(roots);
	return roots;
}

/** Depth-first flattening of a forest back into a flat list (pickers, search). */
export function flattenCategoryTree(forest: CategoryTreeNode[]): CategoryTreeNode[] {
	const out: CategoryTreeNode[] = [];
	const walk = (list: CategoryTreeNode[]) => {
		for (const node of list) {
			out.push(node);
			walk(node.children);
		}
	};
	walk(forest);
	return out;
}

/** A slug's own slug plus every descendant's slug (self included). */
export function descendantSlugs(slug: string, categories: Category[]): string[] {
	const bySlug = new Map(categories.map((c) => [c.slug, c]));
	const byParent = new Map<string, Category[]>();
	for (const c of categories) {
		if (!c.parentId) continue;
		const parent = categories.find((p) => p.id === c.parentId);
		if (!parent) continue;
		if (!byParent.has(parent.slug)) byParent.set(parent.slug, []);
		byParent.get(parent.slug)!.push(c);
	}
	const root = bySlug.get(slug);
	if (!root) return [slug];
	const out: string[] = [slug];
	const walk = (s: string) => {
		for (const child of byParent.get(s) ?? []) {
			out.push(child.slug);
			walk(child.slug);
		}
	};
	walk(slug);
	return out;
}

/**
 * Expand a user's selected slugs into the full slug set to send as
 * `TransactionFilter.categorySlugs` — selecting a parent includes its
 * children (§5/§6), so a transaction labelled with a child category still
 * matches a filter on the parent. De-duplicated.
 */
export function expandSelectedSlugs(selected: string[], categories: Category[]): string[] {
	const expanded = new Set<string>();
	for (const slug of selected) {
		for (const s of descendantSlugs(slug, categories)) expanded.add(s);
	}
	return Array.from(expanded);
}

/** Non-archived categories only, for pickers/filters where archiving hides a category (§5). */
export function activeCategories(categories: Category[]): Category[] {
	return categories.filter((c) => !c.archived);
}

/**
 * Filter a forest by a free-text query, case-insensitively against each
 * node's `name` and `slug`. A node is kept if it matches directly or has a
 * matching descendant, so every match keeps its ancestors; a directly
 * matching node keeps its full subtree. Blank query returns the forest as-is.
 */
export function filterCategoryTree(forest: CategoryTreeNode[], query: string): CategoryTreeNode[] {
	const needle = query.trim().toLowerCase();
	if (!needle) return forest;
	const walk = (list: CategoryTreeNode[]): CategoryTreeNode[] =>
		list.flatMap((node) => {
			const direct =
				node.name.toLowerCase().includes(needle) || node.slug.toLowerCase().includes(needle);
			if (direct) return [node];
			const children = walk(node.children);
			return children.length > 0 ? [{ ...node, children }] : [];
		});
	return walk(forest);
}

export interface CategoryOptionGroup {
	groupLabel: string;
	options: { slug: string; label: string }[];
}

/**
 * Narrow grouped `<select>` options by a free-text query. Case-insensitive
 * substring match against the option's label (full `Parent / Child` path) and
 * its slug; the query is trimmed and a blank query returns everything. Groups
 * left without a matching option are dropped.
 */
export function filterOptionGroups(
	groups: CategoryOptionGroup[],
	query: string
): CategoryOptionGroup[] {
	const needle = query.trim().toLowerCase();
	if (!needle) return groups;
	return groups
		.map((group) => ({
			...group,
			options: group.options.filter(
				(o) => o.label.toLowerCase().includes(needle) || o.slug.toLowerCase().includes(needle)
			)
		}))
		.filter((group) => group.options.length > 0);
}

/**
 * Flat categories -> `Parent / Child`-labelled options grouped by their
 * top-level ancestor, the shape `SearchMenu` takes. Archived categories are
 * left out (they can't be newly assigned).
 */
export function categoryOptionGroups(categories: Category[]): CategoryOptionGroup[] {
	const byId = new Map(categories.map((c) => [c.id, c]));
	function pathOf(cat: Category): string {
		const parent = cat.parentId ? byId.get(cat.parentId) : undefined;
		return parent ? `${pathOf(parent)} / ${cat.name}` : cat.name;
	}
	const groups = new Map<string, { slug: string; label: string }[]>();
	for (const cat of categories) {
		if (cat.archived) continue;
		const label = pathOf(cat);
		const top = label.split(' / ')[0];
		const list = groups.get(top) ?? [];
		list.push({ slug: cat.slug, label });
		groups.set(top, list);
	}
	return [...groups.entries()].map(([groupLabel, options]) => ({
		groupLabel,
		options: options.sort((a, b) => a.label.localeCompare(b.label))
	}));
}
