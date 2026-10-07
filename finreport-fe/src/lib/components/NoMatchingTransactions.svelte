<script lang="ts">
	import Button from './Button.svelte';

	interface Props {
		/** Whether any filter is narrowing the list; without one the period is simply empty. */
		filtered: boolean;
		onclear: () => void;
	}

	let { filtered, onclear }: Props = $props();
</script>

<div class="flex flex-col items-center gap-3 py-8 text-center" role="status">
	{#if filtered}
		<p class="text-sm font-medium text-slate-700">No transactions match these filters.</p>
		<p class="text-sm text-slate-500">Loosen or remove a filter to see more.</p>
		<Button variant="secondary" onclick={onclear}>Clear all filters</Button>
	{:else}
		<p class="text-sm text-slate-500">No transactions in this period.</p>
	{/if}
</div>
