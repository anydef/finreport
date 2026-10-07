<script lang="ts">
	import Button from './Button.svelte';

	interface Props {
		offset: number;
		limit: number;
		totalCount: number;
		onchange: (offset: number) => void;
	}

	let { offset, limit, totalCount, onchange }: Props = $props();

	const page = $derived(Math.floor(offset / limit) + 1);
	const pageCount = $derived(Math.max(1, Math.ceil(totalCount / limit)));
	const hasPrev = $derived(offset > 0);
	const hasNext = $derived(offset + limit < totalCount);
</script>

<nav class="flex items-center justify-between pt-3 text-sm text-slate-600" aria-label="Pagination">
	<p>Page {page} of {pageCount} ({totalCount} total)</p>
	<div class="flex gap-2">
		<Button
			variant="ghost"
			disabled={!hasPrev}
			onclick={() => onchange(Math.max(0, offset - limit))}
		>
			Previous
		</Button>
		<Button variant="ghost" disabled={!hasNext} onclick={() => onchange(offset + limit)}>
			Next
		</Button>
	</div>
</nav>
