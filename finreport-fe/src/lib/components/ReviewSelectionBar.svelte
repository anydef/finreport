<script lang="ts">
	/**
	 * Action bar for the review queue's selection. Rendered only while
	 * something is selected. "Find similar" is offered for exactly one
	 * selection; otherwise it is disabled with the reason spelled out.
	 */
	import type { SimilarTarget } from '$lib/bulkSelection';

	interface Props {
		count: number;
		allMatching: boolean;
		similar: SimilarTarget;
		onAssignCategory: () => void;
		onAssignTags: () => void;
		onFindSimilar: () => void;
		onClear: () => void;
	}

	let {
		count,
		allMatching,
		similar,
		onAssignCategory,
		onAssignTags,
		onFindSimilar,
		onClear
	}: Props = $props();

	const btn =
		'focus-visible:outline-brand rounded-md px-3 py-1 text-xs font-medium focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 disabled:cursor-not-allowed disabled:opacity-50';
	const similarHint = $derived(
		similar.kind === 'no-key'
			? 'This transaction has no counterparty key yet, so there is nothing to match on.'
			: similar.kind === 'needs-one'
				? 'Select exactly one transaction to find others from the same counterparty.'
				: ''
	);
</script>

<div
	data-testid="review-bulk-bar"
	class="sticky top-0 z-10 flex flex-wrap items-center gap-2 rounded-md border border-slate-200 bg-slate-50 px-3 py-2"
>
	<span role="status" class="text-sm font-medium text-slate-900">
		{allMatching ? `All ${count} held selected` : `${count} selected`}
	</span>
	<button
		type="button"
		onclick={onAssignCategory}
		class="{btn} bg-brand hover:bg-brand/90 cursor-pointer text-white"
	>
		Assign category
	</button>
	<button
		type="button"
		onclick={onAssignTags}
		class="{btn} bg-brand hover:bg-brand/90 cursor-pointer text-white"
	>
		Assign tags
	</button>
	<button
		type="button"
		disabled={similar.kind !== 'ready'}
		title={similarHint || undefined}
		onclick={onFindSimilar}
		class="{btn} cursor-pointer bg-slate-100 text-slate-700 hover:bg-slate-200"
	>
		Find similar
	</button>
	<button
		type="button"
		onclick={onClear}
		class="{btn} cursor-pointer bg-slate-100 text-slate-700 hover:bg-slate-200"
	>
		Clear selection
	</button>
	{#if similarHint}
		<span data-testid="similar-hint" class="text-xs text-slate-500">{similarHint}</span>
	{/if}
</div>
