<script lang="ts">
	/**
	 * Dashboard category-breakdown card (§6): horizontal bars by top-level
	 * category for the selected period; clicking a bar narrows the
	 * transaction list (the owning page builds the `categorySlugs` filter
	 * from the clicked bar's `slug`). Plain Tailwind div bars rather than a
	 * LayerChart chart — this is a ranked list with a width-encoded magnitude,
	 * not an axis-based chart, and it needs no `{#if browser}` guard.
	 */
	import { formatAmount } from '$lib/format';
	import { maxBarAmount, shapeCategoryBreakdown, type BreakdownBar } from '$lib/breakdownShaping';
	import type { CategoryBreakdown } from '$lib/graphql/types';

	interface Props {
		breakdown: CategoryBreakdown;
		onSelect?: (bar: BreakdownBar) => void;
	}

	let { breakdown, onSelect }: Props = $props();

	const bars = $derived(shapeCategoryBreakdown(breakdown));
	const maxAmount = $derived(maxBarAmount(bars));

	function barColor(bar: BreakdownBar): string {
		if (bar.kind === 'uncategorized') return 'bg-slate-300';
		if (bar.kind === 'needs-review') return 'bg-amber-300';
		return 'bg-[var(--color-brand)]';
	}
</script>

{#if bars.length === 0}
	<p class="py-8 text-center text-sm text-slate-500">No categorized spending in this period.</p>
{:else}
	<ul class="flex flex-col gap-2">
		{#each bars as bar (bar.key)}
			<li>
				<button
					type="button"
					class="flex w-full flex-col gap-1 rounded px-1 py-1 text-left hover:bg-slate-50 disabled:cursor-default"
					disabled={!onSelect || bar.kind !== 'category'}
					onclick={() => bar.kind === 'category' && onSelect?.(bar)}
				>
					<span class="flex items-center justify-between text-sm">
						<span class="font-medium text-slate-700">
							{bar.label}
							{#if bar.kind !== 'category'}
								<span class="text-xs font-normal text-slate-400">
									({bar.kind === 'uncategorized' ? 'no label' : 'held'})
								</span>
							{/if}
						</span>
						<span class="text-slate-600">
							{formatAmount(String(bar.amount), breakdown.currency)} · {(bar.share * 100).toFixed(
								0
							)}%
						</span>
					</span>
					<span class="h-2 w-full overflow-hidden rounded-full bg-slate-100">
						<span
							class="block h-full rounded-full {barColor(bar)}"
							style="width: {maxAmount > 0 ? (bar.amount / maxAmount) * 100 : 0}%"
						></span>
					</span>
				</button>
			</li>
		{/each}
	</ul>
{/if}
