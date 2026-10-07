<script lang="ts">
	import { LineChart } from 'layerchart';
	import { parseDateInputValue } from '$lib/period';
	import type { CumulativePoint } from '$lib/goalsView';

	interface Props {
		points: CumulativePoint[];
		currency: string;
		goalName: string;
		/** Last day of the goal range, so both lines span the whole range. */
		rangeEnd: string;
	}

	let { points, currency, goalName, rangeEnd }: Props = $props();

	const data = $derived.by(() => {
		const rows = points.map((p) => ({ ...p, date: parseDateInputValue(p.date) }));
		const last = rows[rows.length - 1];
		// Hold the running total flat to the range end so both lines share one x extent.
		const end = parseDateInputValue(rangeEnd);
		if (last && last.date.getTime() < end.getTime()) rows.push({ ...last, date: end });
		return rows;
	});
</script>

<div
	role="img"
	aria-label="{goalName}: cumulative total over time against the budget, in {currency}. See the totals above for the exact figures."
	class="h-72 w-full"
>
	{#if points.length === 0}
		<p class="flex h-full items-center justify-center text-sm text-slate-500">
			Nothing to plot yet.
		</p>
	{:else}
		<LineChart
			{data}
			x="date"
			yDomain={[0, null]}
			series={[
				{ key: 'total', value: 'total', label: 'Total', color: 'var(--color-net)' },
				{ key: 'budget', value: 'budget', label: 'Budget', color: 'var(--color-spending)' }
			]}
			props={{
				yAxis: { label: `Amount (${currency})`, format: (v: number) => v.toLocaleString() },
				legend: { placement: 'bottom' }
			}}
		/>
	{/if}
</div>
