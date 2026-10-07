<script lang="ts">
	/**
	 * Single-category picker (§6, §10: "reused by [the admin pages] later").
	 * Props kept minimal and backend-agnostic — the caller owns loading
	 * `categories` and committing the choice (e.g. `setTransactionCategory`,
	 * a split row, a rule's target category):
	 *
	 * - `categories`: the flat list from the `categories` query.
	 * - `value`: the currently-picked slug, or `null` for none.
	 * - `onchange(slug)`: fired with the newly picked slug.
	 * - `label`/`id`: for an associated `<label>`.
	 */
	import Tree from './Tree.svelte';
	import { activeCategories, buildCategoryTree, filterCategoryTree } from '$lib/categoryTree';
	import type { Category } from '$lib/graphql/types';

	interface Props {
		categories: Category[];
		value: string | null;
		onchange: (slug: string) => void;
		label?: string;
		id?: string;
	}

	let { categories, value, onchange, label, id }: Props = $props();

	const tree = $derived(buildCategoryTree(activeCategories(categories)));
	let query = $state('');
	const visibleTree = $derived(filterCategoryTree(tree, query));
	const selected = $derived(value ? [value] : []);
</script>

<div {id} class="rounded-md border border-slate-200 p-2">
	{#if label}
		<p class="mb-1 text-xs font-medium text-slate-500">{label}</p>
	{/if}
	{#if tree.length === 0}
		<p class="text-sm text-slate-400">No categories available.</p>
	{:else}
		<input
			type="search"
			bind:value={query}
			placeholder="Search categories"
			aria-label={label ? `Search ${label}` : 'Search categories'}
			class="focus-visible:outline-brand mb-2 w-full rounded-md border border-slate-300 px-2 py-1 text-sm focus-visible:outline focus-visible:outline-2"
		/>
		{#if visibleTree.length === 0}
			<p class="text-sm text-slate-400">No categories match your search.</p>
		{:else}
			<Tree nodes={visibleTree} {selected} mode="single" onToggle={onchange} />
		{/if}
	{/if}
</div>
