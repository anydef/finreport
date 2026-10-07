<script lang="ts">
	/**
	 * Read-only tag chips for a transaction row (§5). Click a chip's `×` to
	 * remove it; the owning cell/editor decides what happens next (optimistic
	 * update + mutation).
	 */
	import Badge from './Badge.svelte';

	interface Props {
		tags: string[];
		onRemove?: (tag: string) => void;
	}

	let { tags, onRemove }: Props = $props();
</script>

{#if tags.length > 0}
	<div class="flex flex-wrap items-center gap-1">
		{#each tags as tag (tag)}
			<span class="inline-flex items-center gap-1">
				<Badge text={tag} variant="neutral" />
				{#if onRemove}
					<button
						type="button"
						class="text-xs text-slate-400 hover:text-slate-700"
						aria-label={`Remove tag ${tag}`}
						onclick={() => onRemove(tag)}
					>
						×
					</button>
				{/if}
			</span>
		{/each}
	</div>
{/if}
