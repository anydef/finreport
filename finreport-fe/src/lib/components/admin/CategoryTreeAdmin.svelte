<script lang="ts">
	/**
	 * `/admin/categories` tree (§5, §6, §10 WP6): recursive rendering of
	 * `buildCategoryTree`'s output, depth-3 nodes refusing children in the UI
	 * (`canHaveChildren`, mirroring the API's own depth check), with
	 * create/rename/archive actions. There is no `moveCategory` mutation in
	 * the SDL (§5), so "move" is intentionally not offered here.
	 */
	import Button from '../Button.svelte';
	import { canHaveChildren, type CategoryNode } from '$lib/rulesView';

	interface Props {
		nodes: CategoryNode[];
		oncreatechild: (parent: CategoryNode) => void;
		onrename: (node: CategoryNode) => void;
		onarchive: (node: CategoryNode) => void;
	}

	let { nodes, oncreatechild, onrename, onarchive }: Props = $props();

	const kindStyles: Record<string, string> = {
		INCOME: 'bg-[color:var(--color-income)]/10 text-[color:var(--color-income)]',
		EXPENSE: 'bg-[color:var(--color-spending)]/10 text-[color:var(--color-spending)]',
		TRANSFER: 'bg-slate-200 text-slate-600',
		SAVING: 'bg-emerald-100 text-emerald-700'
	};
</script>

{#snippet kindBadge(kind: string)}
	<span
		class={`rounded-full px-2 py-0.5 text-xs font-medium ${kindStyles[kind] ?? 'bg-slate-100 text-slate-600'}`}
	>
		{kind.toLowerCase()}
	</span>
{/snippet}

{#snippet tree(list: CategoryNode[])}
	<ul class="flex flex-col gap-1">
		{#each list as node (node.id)}
			<li>
				<div
					class="flex items-center justify-between gap-2 rounded-md px-2 py-1.5 hover:bg-slate-50"
					class:opacity-50={node.archived}
				>
					<div class="flex items-center gap-2">
						<span class="text-sm font-medium text-slate-900">{node.name}</span>
						{@render kindBadge(node.kind)}
						{#if node.archived}
							<span class="text-xs text-slate-400">archived</span>
						{/if}
					</div>
					<div class="flex gap-1">
						{#if canHaveChildren(node) && !node.archived}
							<Button variant="ghost" onclick={() => oncreatechild(node)}>Add child</Button>
						{/if}
						<Button variant="ghost" onclick={() => onrename(node)}>Rename</Button>
						{#if !node.archived}
							<Button variant="ghost" onclick={() => onarchive(node)}>Archive</Button>
						{/if}
					</div>
				</div>
				{#if node.children.length > 0}
					<div class="ml-5 border-l border-slate-200 pl-3">
						{@render tree(node.children)}
					</div>
				{/if}
			</li>
		{/each}
	</ul>
{/snippet}

{#if nodes.length === 0}
	<p class="py-8 text-center text-sm text-slate-500">No categories yet.</p>
{:else}
	{@render tree(nodes)}
{/if}
