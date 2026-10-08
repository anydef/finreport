<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import Card from '$lib/components/Card.svelte';
	import Field from '$lib/components/Field.svelte';
	import TransactionTable from '$lib/components/TransactionTable.svelte';
	import TransactionFilters from '$lib/components/TransactionFilters.svelte';
	import NoMatchingTransactions from '$lib/components/NoMatchingTransactions.svelte';
	import Pagination from '$lib/components/Pagination.svelte';
	import { PERIOD_PRESETS, type PeriodPresetId } from '$lib/period';
	import {
		clearedFilters,
		hasActiveFilters,
		writePanelFilters,
		type PanelFilters
	} from '$lib/transactionFilters';
	import { writeSort } from '$lib/transactionSort';
	import type { TransactionSort } from '$lib/graphql/types';
	import type { PageData } from './$types';

	let { data }: { data: PageData } = $props();

	let preset = $state(data.preset);
	$effect(() => {
		preset = data.preset;
	});

	function go(params: URLSearchParams) {
		goto(`${page.url.pathname}?${params.toString()}`, { keepFocus: true, noScroll: true });
	}

	function onFiltersChange(next: PanelFilters) {
		const params = writePanelFilters(next, page.url.searchParams);
		params.delete('offset');
		go(params);
	}

	function onPeriodChange() {
		const params = new URLSearchParams(page.url.searchParams);
		params.delete('offset');
		if (preset === 'this-month') params.delete('preset');
		else params.set('preset', preset as PeriodPresetId);
		go(params);
	}

	function onSort(next: TransactionSort) {
		go(writeSort(next, page.url.searchParams));
	}

	function onPageChange(offset: number) {
		const params = new URLSearchParams(page.url.searchParams);
		if (offset) params.set('offset', String(offset));
		else params.delete('offset');
		go(params);
	}

	const filtered = $derived(hasActiveFilters(data.panel));
</script>

<svelte:head>
	<title>Transactions · finreport</title>
</svelte:head>

<div class="flex flex-col gap-6">
	<TransactionFilters
		value={data.panel}
		accounts={data.accounts}
		categories={data.categories}
		tags={data.allTags}
		matchCount={data.transactions?.totalCount}
		scopeNote="Filters apply to the {data.start} to {data.end} period selected above."
		onchange={onFiltersChange}
	>
		{#snippet lead()}
			<Field label="Period" for="tx-preset">
				<select
					id="tx-preset"
					bind:value={preset}
					onchange={onPeriodChange}
					class="focus-visible:outline-brand rounded-md border border-slate-300 px-2 py-1.5 text-sm focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-1"
				>
					{#each PERIOD_PRESETS.filter((p) => p.id !== 'custom') as p (p.id)}
						<option value={p.id}>{p.label}</option>
					{/each}
				</select>
			</Field>
		{/snippet}
	</TransactionFilters>

	{#if data.error}
		<p role="alert" class="text-sm text-[var(--color-spending)]">
			Failed to load transactions from the GraphQL API.
		</p>
	{:else if data.transactions}
		<Card>
			{#if data.transactions.totalCount === 0}
				<NoMatchingTransactions {filtered} onclear={() => onFiltersChange(clearedFilters())} />
			{:else}
				<TransactionTable
					transactions={data.transactions.items}
					filter={data.transactionFilter}
					totalCount={data.transactions.totalCount}
					sort={data.sort}
					onsort={onSort}
					currency={data.accounts[0]?.currency ?? 'EUR'}
				/>
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
