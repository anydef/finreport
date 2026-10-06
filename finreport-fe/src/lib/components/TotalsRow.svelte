<script lang="ts">
	import { formatAmount } from '$lib/format';
	import type { CashflowTotals } from '$lib/graphql/types';
	import Card from './Card.svelte';

	interface Props {
		totals: CashflowTotals;
		currency: string;
	}

	let { totals, currency }: Props = $props();

	const rows = $derived([
		{ label: 'Income', value: totals.income, tone: 'text-[var(--color-income)]' },
		{ label: 'Spending', value: `-${totals.spending}`, tone: 'text-[var(--color-spending)]' },
		{ label: 'Net', value: totals.net, tone: 'text-[var(--color-net)]' }
	]);
</script>

<div class="grid grid-cols-1 gap-4 sm:grid-cols-3">
	{#each rows as row (row.label)}
		<Card>
			<p class="text-sm text-slate-500">{row.label}</p>
			<p class="mt-1 text-2xl font-semibold {row.tone}">{formatAmount(row.value, currency)}</p>
		</Card>
	{/each}
	<p class="col-span-full text-sm text-slate-500">
		{totals.transactionCount} transaction{totals.transactionCount === 1 ? '' : 's'} in this period
	</p>
</div>
