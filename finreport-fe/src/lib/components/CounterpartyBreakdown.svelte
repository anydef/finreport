<script lang="ts">
	/**
	 * Dashboard "Counterparty" breakdown: a flat list of signed net amounts per
	 * merchant. No share/percentage (rows mix income and spending) and no
	 * hierarchy. A merchant row links to `/transactions` for that merchant; the
	 * no-key bucket and the tail `Other` row are plain, non-clickable rows.
	 * All decisions live in `counterpartyView.ts`.
	 */
	import {
		counterpartyHref,
		shapeCounterpartyBreakdown,
		transactionCountLabel,
		type AmountTone,
		type DrillScope
	} from '$lib/counterpartyView';
	import type { CounterpartyBreakdown } from '$lib/graphql/types';

	interface Props {
		breakdown: CounterpartyBreakdown;
		scope: DrillScope;
	}

	let { breakdown, scope }: Props = $props();

	const rows = $derived(shapeCounterpartyBreakdown(breakdown));

	const toneClass: Record<AmountTone, string> = {
		income: 'text-[var(--color-income)]',
		spending: 'text-[var(--color-spending)]',
		neutral: 'text-slate-600'
	};
	const focusRing =
		'focus-visible:outline-brand focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2';
</script>

{#if rows.length === 0}
	<p class="py-8 text-center text-sm text-slate-500">No transactions in this period.</p>
{:else}
	<p class="mb-2 text-xs text-slate-400">
		Net per counterparty: income is positive, spending negative, refunds cancel charges. Click a
		merchant to list its transactions.
	</p>
	<ul class="flex flex-col divide-y divide-slate-100">
		{#each rows as row (row.key)}
			{@const href = counterpartyHref(row, scope)}
			<li class={row.kind === 'other' ? 'bg-slate-50 italic' : ''}>
				{#snippet body()}
					<span class="flex min-w-0 flex-col">
						<span class="truncate text-sm font-medium text-slate-700">
							{row.label}
							{#if row.kind === 'no-key'}
								<span class="text-xs font-normal text-slate-400">(not drillable)</span>
							{:else if row.kind === 'other'}
								<span class="text-xs font-normal text-slate-400">(everything beyond the top)</span>
							{/if}
						</span>
						<span class="text-xs text-slate-400">{transactionCountLabel(row.transactionCount)}</span
						>
					</span>
					<span class="shrink-0 text-sm font-medium tabular-nums {toneClass[row.tone]}">
						{row.amountText}
					</span>
				{/snippet}
				{#if href}
					<a
						{href}
						class="flex items-center justify-between gap-3 rounded px-1 py-2 hover:bg-slate-50 {focusRing}"
						title="Show transactions: {row.label}"
					>
						{@render body()}
					</a>
				{:else}
					<div class="flex items-center justify-between gap-3 px-1 py-2">
						{@render body()}
					</div>
				{/if}
			</li>
		{/each}
	</ul>
{/if}
