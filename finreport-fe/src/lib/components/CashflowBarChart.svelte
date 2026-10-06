<script lang="ts">
	import { BarChart } from 'layerchart';
	import { scaleBand } from 'd3-scale';
	import type { CashflowBarDatum } from '$lib/chartShaping';

	interface Props {
		bars: CashflowBarDatum[];
		currency: string;
		periodLabel: string;
		onBarClick?: (bar: CashflowBarDatum) => void;
	}

	let { bars, currency, periodLabel, onBarClick }: Props = $props();
</script>

<div
	role="img"
	aria-label="Income and spending by period for {periodLabel}, in {currency}. See the transaction list below for the underlying data."
	class="h-72 w-full"
>
	{#if bars.length === 0}
		<p class="flex h-full items-center justify-center text-sm text-slate-500">
			No transactions in this period.
		</p>
	{:else}
		<BarChart
			data={bars}
			x="label"
			xScale={scaleBand().paddingInner(0.3).paddingOuter(0.1)}
			series={[
				{ key: 'income', value: 'income', color: 'var(--color-income)' },
				{ key: 'spending', value: 'spending', color: 'var(--color-spending)' }
			]}
			seriesLayout="group"
			onBarClick={(_event, detail) => onBarClick?.(detail.data as CashflowBarDatum)}
			props={{
				xAxis: { label: 'Period' },
				yAxis: { label: `Amount (${currency})`, format: (v: number) => v.toLocaleString() },
				legend: { placement: 'bottom' }
			}}
		/>
	{/if}
</div>
