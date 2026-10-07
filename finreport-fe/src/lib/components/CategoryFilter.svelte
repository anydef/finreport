<script lang="ts">
	/**
	 * Category multi-select filter for the transaction list (§6): a category
	 * tree (selecting a parent includes its children, done by the caller via
	 * `expandSelectedSlugs` before sending `categorySlugs` to the query) plus
	 * the *uncategorized* / *needs review* toggles. Follows the same
	 * `bind:`+`onchange()` convention as `PeriodSelector` so the owning page
	 * controls URL persistence.
	 */
	import Tree from './Tree.svelte';
	import { activeCategories, buildCategoryTree } from '$lib/categoryTree';
	import type { Category } from '$lib/graphql/types';

	interface Props {
		categories: Category[];
		selectedSlugs: string[];
		uncategorized: boolean;
		needsReview: boolean;
		onchange: () => void;
	}

	let {
		categories,
		selectedSlugs = $bindable(),
		uncategorized = $bindable(),
		needsReview = $bindable(),
		onchange
	}: Props = $props();

	const tree = $derived(buildCategoryTree(activeCategories(categories)));

	function toggleSlug(slug: string) {
		selectedSlugs = selectedSlugs.includes(slug)
			? selectedSlugs.filter((s) => s !== slug)
			: [...selectedSlugs, slug];
		onchange();
	}

	function toggleUncategorized() {
		uncategorized = !uncategorized;
		onchange();
	}

	function toggleNeedsReview() {
		needsReview = !needsReview;
		onchange();
	}
</script>

<div class="flex flex-col gap-2">
	<p class="text-xs font-medium text-slate-500">Category</p>
	<label class="flex items-center gap-2 text-sm">
		<input type="checkbox" checked={uncategorized} onchange={toggleUncategorized} />
		Uncategorized
	</label>
	<label class="flex items-center gap-2 text-sm">
		<input type="checkbox" checked={needsReview} onchange={toggleNeedsReview} />
		Needs review
	</label>
	{#if tree.length > 0}
		<div class="max-h-48 overflow-y-auto rounded-md border border-slate-200 p-2">
			<Tree nodes={tree} selected={selectedSlugs} mode="multi" onToggle={toggleSlug} />
		</div>
	{/if}
</div>
