<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { browser } from '$app/environment';
	import Card from '$lib/components/Card.svelte';
	import PeriodSelector from '$lib/components/PeriodSelector.svelte';
	import TotalsRow from '$lib/components/TotalsRow.svelte';
	import CashflowBarChart from '$lib/components/CashflowBarChart.svelte';
	import CashflowSankey from '$lib/components/CashflowSankey.svelte';
	import CategoryBreakdown from '$lib/components/CategoryBreakdown.svelte';
	import TransactionTable from '$lib/components/TransactionTable.svelte';
	import Pagination from '$lib/components/Pagination.svelte';
	import Button from '$lib/components/Button.svelte';
	import type { BreakdownBar } from '$lib/breakdownShaping';
	import {
		drilldownFilterForLink,
		drilldownFilterForNode,
		shapeCashflowBars,
		shapeCashflowGraph,
		type CashflowBarDatum,
		type ShapedSankeyNode
	} from '$lib/chartShaping';
	import { defaultGranularity } from '$lib/period';
	import type { PageData } from './$types';

	let { data }: { data: PageData } = $props();

	let preset = $state(data.preset);
	let start = $state(data.start);
	let end = $state(data.end);
	let granularity = $state(data.granularity);

	$effect(() => {
		preset = data.preset;
		start = data.start;
		end = data.end;
		granularity = data.granularity;
	});

	interface NavOverrides {
		txStart?: string;
		txEnd?: string;
		accountIds?: string[];
		counterpartyNames?: string[];
		hasCounterparty?: boolean;
		categorySlugs?: string[];
		uncategorized?: boolean;
		offset?: number;
		resetDrilldown?: boolean;
		sankeyDimension?: 'counterparty' | 'category';
	}

	function navigate(opts: NavOverrides = {}) {
		const base = opts.resetDrilldown
			? {}
			: {
					txStart: data.txStart,
					txEnd: data.txEnd,
					accountIds: data.drilldown.accountIds,
					counterpartyNames: data.drilldown.counterpartyNames,
					hasCounterparty: data.drilldown.hasCounterparty,
					categorySlugs: data.drilldown.categorySlugs,
					uncategorized: data.drilldown.uncategorized,
					offset: data.offset
				};
		const merged = { ...base, ...opts };
		const sankeyDimension = opts.sankeyDimension ?? data.sankeyDimension;

		const params = new URLSearchParams();
		if (preset !== 'this-month') params.set('preset', preset);
		if (preset === 'custom') {
			params.set('start', start);
			params.set('end', end);
		}
		if (granularity !== defaultGranularity({ start, end })) params.set('granularity', granularity);
		if (sankeyDimension !== 'counterparty') params.set('sankey', sankeyDimension);
		if (merged.txStart && merged.txStart !== start) params.set('txStart', merged.txStart);
		if (merged.txEnd && merged.txEnd !== end) params.set('txEnd', merged.txEnd);
		if (merged.accountIds?.length) params.set('accountIds', merged.accountIds.join(','));
		if (merged.counterpartyNames?.length)
			params.set('counterpartyNames', merged.counterpartyNames.join(','));
		if (merged.hasCounterparty === false) params.set('hasCounterparty', 'false');
		if (merged.categorySlugs?.length) params.set('categorySlugs', merged.categorySlugs.join(','));
		if (merged.uncategorized) params.set('uncategorized', 'true');
		if (merged.offset) params.set('offset', String(merged.offset));

		goto(`${page.url.pathname}?${params.toString()}`, { keepFocus: true, noScroll: true });
	}

	function applyPeriod() {
		navigate({ resetDrilldown: true });
	}

	function onBarClick(bar: CashflowBarDatum) {
		navigate({ txStart: bar.start, txEnd: bar.end, offset: 0, resetDrilldown: true });
	}

	function onNodeClick(node: ShapedSankeyNode) {
		const filter = drilldownFilterForNode(node);
		if (!filter) return;
		navigate({
			accountIds: filter.accountIds ?? undefined,
			counterpartyNames: filter.counterpartyNames ?? undefined,
			hasCounterparty: filter.hasCounterparty ?? undefined,
			categorySlugs: filter.categorySlugs ?? undefined,
			uncategorized: filter.uncategorized ?? undefined,
			offset: 0,
			resetDrilldown: true
		});
	}

	function onLinkClick(sourceId: string, targetId: string) {
		if (!graph) return;
		const source = graph.nodes.find((n) => n.id === sourceId);
		const target = graph.nodes.find((n) => n.id === targetId);
		const filter = drilldownFilterForLink(source, target);
		if (!filter) return;
		navigate({
			accountIds: filter.accountIds ?? undefined,
			counterpartyNames: filter.counterpartyNames ?? undefined,
			hasCounterparty: filter.hasCounterparty ?? undefined,
			categorySlugs: filter.categorySlugs ?? undefined,
			uncategorized: filter.uncategorized ?? undefined,
			offset: 0,
			resetDrilldown: true
		});
	}

	function clearDrilldown() {
		navigate({ resetDrilldown: true });
	}

	function onPageChange(offset: number) {
		navigate({ offset });
	}

	function onBreakdownSelect(bar: BreakdownBar) {
		if (!bar.slug) return;
		navigate({ categorySlugs: [bar.slug], offset: 0, resetDrilldown: true });
	}

	function onSankeyDimensionChange(dimension: 'counterparty' | 'category') {
		navigate({ sankeyDimension: dimension, resetDrilldown: true });
	}

	const bars = $derived(data.summary ? shapeCashflowBars(data.summary, data.granularity) : []);
	const graph = $derived(data.graph ? shapeCashflowGraph(data.graph) : undefined);
	const periodLabel = $derived(`${data.start} to ${data.end}`);
	const hasDrilldown = $derived(
		Boolean(
			data.drilldown.accountIds?.length ||
				data.drilldown.counterpartyNames?.length ||
				data.drilldown.hasCounterparty === false ||
				data.drilldown.categorySlugs?.length ||
				data.drilldown.uncategorized ||
				data.txStart !== data.start ||
				data.txEnd !== data.end
		)
	);
</script>

<svelte:head>
	<title>Dashboard · finreport</title>
</svelte:head>

<div class="flex flex-col gap-6">
	<Card>
		<PeriodSelector
			bind:preset
			bind:startDate={start}
			bind:endDate={end}
			bind:granularity
			onchange={applyPeriod}
		/>
	</Card>

	{#if data.error}
		<p role="alert" class="text-sm text-[var(--color-spending)]">
			Failed to load dashboard data from the GraphQL API.
		</p>
	{:else if data.summary}
		<TotalsRow totals={data.summary.total} currency={data.summary.currency} />

		<Card title="Income vs. spending">
			{#if browser}
				<CashflowBarChart {bars} currency={data.summary.currency} {periodLabel} {onBarClick} />
			{:else}
				<p class="flex h-72 items-center justify-center text-sm text-slate-400">Loading chart…</p>
			{/if}
		</Card>

		{#if graph}
			<Card title="Cash flow">
				<div class="mb-3 flex justify-end gap-1" role="group" aria-label="Sankey grouping">
					<Button
						variant={data.sankeyDimension === 'counterparty' ? 'primary' : 'ghost'}
						onclick={() => onSankeyDimensionChange('counterparty')}
					>
						Counterparty
					</Button>
					<Button
						variant={data.sankeyDimension === 'category' ? 'primary' : 'ghost'}
						onclick={() => onSankeyDimensionChange('category')}
					>
						Category
					</Button>
				</div>
				{#if browser}
					<CashflowSankey {graph} {periodLabel} {onNodeClick} {onLinkClick} />
				{:else}
					<p class="flex h-96 items-center justify-center text-sm text-slate-400">Loading chart…</p>
				{/if}
			</Card>
		{/if}

		{#if data.breakdown}
			<Card title="Spending by category">
				<CategoryBreakdown breakdown={data.breakdown} onSelect={onBreakdownSelect} />
			</Card>
		{/if}

		<Card title="Transactions">
			{#if hasDrilldown}
				<div class="mb-3 flex items-center justify-between">
					<p class="text-sm text-slate-500">Filtered by your chart selection.</p>
					<Button variant="ghost" onclick={clearDrilldown}>Clear filter</Button>
				</div>
			{/if}
			{#if data.transactions}
				<TransactionTable transactions={data.transactions.items} currency={data.summary.currency} />
				<Pagination
					offset={data.transactions.offset}
					limit={data.transactions.limit}
					totalCount={data.transactions.totalCount}
					onchange={onPageChange}
				/>
			{/if}
		</Card>
	{/if}
</div>
