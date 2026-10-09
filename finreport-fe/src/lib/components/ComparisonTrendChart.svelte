<script lang="ts">
	import { BarChart } from 'layerchart';
	import { scaleBand } from 'd3-scale';
	import type { TrendBar } from '$lib/comparisonView';

	interface Props {
		bars: TrendBar[];
		currency: string;
		onBarClick?: (bar: TrendBar) => void;
	}

	let { bars, currency, onBarClick }: Props = $props();
</script>

<div
	role="img"
	aria-label="Total spending per month in {currency}. The two months being compared are highlighted; the table below has every figure."
	class="h-64 w-full"
>
	{#if bars.length === 0}
		<p class="flex h-full items-center justify-center text-sm text-slate-500">
			No spending in this window.
		</p>
	{:else}
		<BarChart
			data={bars}
			x="label"
			xScale={scaleBand().paddingInner(0.3).paddingOuter(0.1)}
			series={[
				{ key: 'other', label: 'Other months', value: 'other', color: 'var(--color-muted)' },
				{ key: 'base', label: 'Compared from', value: 'base', color: 'var(--color-account)' },
				{ key: 'current', label: 'Compared to', value: 'current', color: 'var(--color-brand)' }
			]}
			seriesLayout="stack"
			onBarClick={(_event, detail) => onBarClick?.(detail.data as TrendBar)}
			props={{
				yAxis: { format: (v: number) => v.toLocaleString() },
				legend: { placement: 'bottom' }
			}}
		/>
	{/if}
</div>
