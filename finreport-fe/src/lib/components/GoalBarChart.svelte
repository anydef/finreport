<script lang="ts">
	import { BarChart } from 'layerchart';
	import { scaleBand } from 'd3-scale';
	import { decimalToNumber } from '$lib/chartShaping';
	import type { GoalBarDatum } from '$lib/goalsView';

	interface Props {
		bars: GoalBarDatum[];
		/** The per-period threshold, drawn as a reference line. */
		threshold: string;
		currency: string;
		goalName: string;
		onBarClick?: (bar: GoalBarDatum) => void;
	}

	let { bars, threshold, currency, goalName, onBarClick }: Props = $props();

	// One series per tone so each bar takes its colour from the shared tokens;
	// a bar only ever has a value in its own tone's series.
	const rows = $derived(
		bars.map((bar) => ({
			label: bar.label,
			good: bar.tone === 'good' ? bar.total : 0,
			bad: bar.tone === 'bad' ? bar.total : 0,
			neutral: bar.tone === 'neutral' ? bar.total : 0
		}))
	);
	const limit = $derived(decimalToNumber(threshold));

	function onClick(label: string) {
		const bar = bars.find((b) => b.label === label);
		if (bar) onBarClick?.(bar);
	}
</script>

<div
	role="img"
	aria-label="{goalName}: total per period against a threshold of {threshold} {currency}. See the period list below for the exact figures."
	class="h-72 w-full"
>
	{#if bars.length === 0}
		<p class="flex h-full items-center justify-center text-sm text-slate-500">
			No periods to show yet.
		</p>
	{:else}
		<BarChart
			data={rows}
			x="label"
			xScale={scaleBand().paddingInner(0.3).paddingOuter(0.1)}
			series={[
				{ key: 'good', value: 'good', color: 'var(--color-income)' },
				{ key: 'bad', value: 'bad', color: 'var(--color-spending)' },
				{ key: 'neutral', value: 'neutral', color: 'var(--color-muted)' }
			]}
			seriesLayout="stack"
			yDomain={[0, null]}
			annotations={[
				{
					type: 'line',
					y: limit,
					label: `Threshold ${limit.toLocaleString()}`,
					labelPlacement: 'top-left',
					props: { line: { class: 'stroke-slate-900 stroke-2 [stroke-dasharray:6_4]' } }
				}
			]}
			onBarClick={(_event, detail) => onClick((detail.data as { label: string }).label)}
			props={{
				xAxis: { label: 'Period' },
				yAxis: { label: `Amount (${currency})`, format: (v: number) => v.toLocaleString() }
			}}
		/>
	{/if}
</div>
