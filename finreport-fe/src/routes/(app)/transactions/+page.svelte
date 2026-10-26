<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import Card from '$lib/components/Card.svelte';
	import Field from '$lib/components/Field.svelte';
	import TransactionTable from '$lib/components/TransactionTable.svelte';
	import TransactionFilters from '$lib/components/TransactionFilters.svelte';
	import NoMatchingTransactions from '$lib/components/NoMatchingTransactions.svelte';
	import ReimbursementNote from '$lib/components/ReimbursementNote.svelte';
	import Button from '$lib/components/Button.svelte';
	import Pagination from '$lib/components/Pagination.svelte';
	import type { PeriodSelection } from '$lib/period';
	import PeriodOptions from '$lib/components/PeriodOptions.svelte';
	import {
		clearedFilters,
		hasActiveFilters,
		writePanelFilters,
		type PanelFilters
	} from '$lib/transactionFilters';
	import {
		COUNTERPARTY_IBANS_PARAM,
		COUNTERPARTY_KEYS_PARAM,
		HAS_COUNTERPARTY_IBAN_PARAM,
		narrowedToLabel
	} from '$lib/counterpartyView';
	import { writeSort } from '$lib/transactionSort';
	import type { TransactionSort } from '$lib/graphql/types';
	import type { PageData } from './$types';

	let { data }: { data: PageData } = $props();

	// Writable `$derived` (Svelte >= 5.25): it follows `data.preset` when the
	// page's data changes, and a local assignment holds until it does. The
	// `$state` + `$effect` pair this replaces did the same thing a frame later,
	// and could clobber a local edit whenever the effect re-ran.
	let preset = $derived(data.preset);

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
		else params.set('preset', preset as PeriodSelection);
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

	function clearMerchant() {
		const params = new URLSearchParams(page.url.searchParams);
		params.delete(COUNTERPARTY_KEYS_PARAM);
		params.delete(COUNTERPARTY_IBANS_PARAM);
		params.delete(HAS_COUNTERPARTY_IBAN_PARAM);
		params.delete('offset');
		go(params);
	}

	/** "Clear filters" also drops the merchant, or an empty list could not be escaped. */
	function onClearAll() {
		const params = writePanelFilters(clearedFilters(), page.url.searchParams);
		params.delete(COUNTERPARTY_KEYS_PARAM);
		params.delete(COUNTERPARTY_IBANS_PARAM);
		params.delete(HAS_COUNTERPARTY_IBAN_PARAM);
		params.delete('offset');
		go(params);
	}

	const merchantNarrowed = $derived(
		data.counterpartyKeys.length > 0 ||
			data.counterpartyIbans.length > 0 ||
			data.hasCounterpartyIban !== undefined
	);
	const filtered = $derived(hasActiveFilters(data.panel) || merchantNarrowed);
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
					<PeriodOptions allowCustom={false} />
				</select>
			</Field>
		{/snippet}
	</TransactionFilters>

	{#if merchantNarrowed}
		<div class="flex items-center justify-between gap-2 text-sm text-slate-600">
			<p>
				Narrowed to merchant: {narrowedToLabel(
					data.counterpartyKeys,
					data.counterpartyIbans,
					data.hasCounterpartyIban
				)}
			</p>
			<Button variant="ghost" onclick={clearMerchant}>Clear merchant</Button>
		</div>
	{/if}

	{#if data.error}
		<p role="alert" class="text-sm text-[var(--color-spending)]">
			Failed to load transactions from the GraphQL API.
		</p>
	{:else if data.transactions}
		<ReimbursementNote
			summary={data.reimbursementSummary}
			currency={data.accounts[0]?.currency ?? 'EUR'}
		/>
		<Card>
			{#if data.transactions.totalCount === 0}
				<NoMatchingTransactions {filtered} onclear={onClearAll} />
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
