<script lang="ts">
	/**
	 * "Needs attention" strip at the top of the dashboard. All-time on purpose
	 * (the period selector below does not move it), and it says so. Nothing to
	 * do renders a quiet single line — no zeroes, no alarm colours.
	 */
	import { attentionItems } from '$lib/attentionView';
	import type { AttentionSummary } from '$lib/graphql/types';

	interface Props {
		summary: AttentionSummary;
		currency: string;
		/** "YYYY-MM-DD", the end of the all-time range the uncategorised link opens. */
		today: string;
	}

	let { summary, currency, today }: Props = $props();

	const items = $derived(attentionItems(summary, currency, today));
</script>

{#if items.length === 0}
	<p
		data-testid="attention-clear"
		class="rounded-lg border border-slate-200 bg-white px-4 py-2 text-sm text-slate-500"
	>
		<span aria-hidden="true">✓</span> Nothing needs attention — every transaction is categorised and
		no labels are held for review.
	</p>
{:else}
	<section
		aria-label="Needs attention"
		data-testid="attention-summary"
		class="rounded-lg border border-amber-200 bg-amber-50 p-4"
	>
		<div class="mb-2 flex flex-wrap items-baseline justify-between gap-2">
			<h2 class="text-sm font-semibold text-amber-900 uppercase">Needs attention</h2>
			<p class="text-xs text-amber-800">All time — not affected by the period below</p>
		</div>
		<ul class="flex flex-wrap gap-3">
			{#each items as item (item.key)}
				<li class="min-w-48 flex-1">
					<a
						href={item.href}
						data-testid="attention-{item.key}"
						class="focus-visible:outline-brand block rounded-md bg-white px-3 py-2 shadow-sm hover:bg-amber-100 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2"
					>
						<span class="block text-2xl font-semibold text-slate-900">{item.count}</span>
						<span class="block text-sm text-slate-700">{item.label}</span>
						<span class="block text-xs text-slate-500">worth {item.worth} net</span>
					</a>
				</li>
			{/each}
		</ul>
	</section>
{/if}
