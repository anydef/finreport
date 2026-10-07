<script lang="ts">
	/**
	 * Generic recursive category tree, shared by `CategoryPicker` (single
	 * select) and `CategoryFilter` (multi select), and reusable read-only by
	 * WP6's admin tree/rule-form category pickers (§10).
	 *
	 * Kept deliberately dumb: it only knows how to render a forest and report
	 * a toggled slug: `nodes`, which slugs are `selected`, a `mode`, and an
	 * `onToggle(slug)` callback. Selection semantics (e.g. "selecting a parent
	 * selects its children") are the caller's responsibility
	 * (`categoryTree.ts`'s `expandSelectedSlugs`), not this component's.
	 */
	import type { CategoryTreeNode } from '$lib/categoryTree';
	import Self from './Tree.svelte';

	interface Props {
		nodes: CategoryTreeNode[];
		selected: string[];
		mode: 'single' | 'multi';
		onToggle: (slug: string) => void;
		/** Internal: indentation level, do not pass from outside. */
		depth?: number;
	}

	let { nodes, selected, mode, onToggle, depth = 0 }: Props = $props();
</script>

<ul class="flex flex-col gap-0.5" style="padding-left: {depth ? '1rem' : '0'}">
	{#each nodes as node (node.id)}
		<li>
			<label class="flex items-center gap-2 rounded px-1 py-0.5 text-sm hover:bg-slate-50">
				<input
					type={mode === 'single' ? 'radio' : 'checkbox'}
					name={mode === 'single' ? 'category-picker' : undefined}
					checked={selected.includes(node.slug)}
					onchange={() => onToggle(node.slug)}
				/>
				<span class:text-slate-400={node.archived}>{node.name}</span>
			</label>
			{#if node.children.length > 0}
				<Self nodes={node.children} {selected} {mode} {onToggle} depth={depth + 1} />
			{/if}
		</li>
	{/each}
</ul>
