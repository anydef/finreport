<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import Card from '$lib/components/Card.svelte';
	import Field from '$lib/components/Field.svelte';
	import TransactionTable from '$lib/components/TransactionTable.svelte';
	import Pagination from '$lib/components/Pagination.svelte';
	import { PERIOD_PRESETS, type PeriodPresetId } from '$lib/period';
	import type { PageData } from './$types';

	let { data }: { data: PageData } = $props();

	let preset = $state(data.preset);
	let accountId = $state(data.accountId ?? '');
	let search = $state(data.search ?? '');

	$effect(() => {
		preset = data.preset;
		accountId = data.accountId ?? '';
		search = data.search ?? '';
	});

	function applyFilters() {
		const params = new URLSearchParams();
		if (preset !== 'this-month') params.set('preset', preset as PeriodPresetId);
		if (accountId) params.set('accountId', accountId);
		if (search) params.set('search', search);
		goto(`${page.url.pathname}?${params.toString()}`, { keepFocus: true, noScroll: true });
	}

	function onPageChange(offset: number) {
		const params = new URLSearchParams(page.url.searchParams);
		if (offset) params.set('offset', String(offset));
		else params.delete('offset');
		goto(`${page.url.pathname}?${params.toString()}`, { keepFocus: true, noScroll: true });
	}
</script>

<svelte:head>
	<title>Transactions · finreport</title>
</svelte:head>

<div class="flex flex-col gap-6">
	<Card>
		<form
			class="flex flex-wrap items-end gap-4"
			onsubmit={(e) => {
				e.preventDefault();
				applyFilters();
			}}
		>
			<Field label="Period" for="tx-preset">
				<select
					id="tx-preset"
					bind:value={preset}
					onchange={applyFilters}
					class="rounded-md border-slate-300 text-sm"
				>
					{#each PERIOD_PRESETS.filter((p) => p.id !== 'custom') as p (p.id)}
						<option value={p.id}>{p.label}</option>
					{/each}
				</select>
			</Field>
			<Field label="Account" for="tx-account">
				<select
					id="tx-account"
					bind:value={accountId}
					onchange={applyFilters}
					class="rounded-md border-slate-300 text-sm"
				>
					<option value="">All accounts</option>
					{#each data.accounts as account (account.id)}
						<option value={account.id}>{account.label ?? account.displayId ?? account.id}</option>
					{/each}
				</select>
			</Field>
			<Field label="Search" for="tx-search">
				<input
					id="tx-search"
					type="search"
					bind:value={search}
					placeholder="Counterparty or description"
					class="rounded-md border-slate-300 text-sm"
				/>
			</Field>
		</form>
	</Card>

	{#if data.error}
		<p role="alert" class="text-sm text-[var(--color-spending)]">
			Failed to load transactions from the GraphQL API.
		</p>
	{:else if data.transactions}
		<Card>
			<TransactionTable
				transactions={data.transactions.items}
				currency={data.accounts[0]?.currency ?? 'EUR'}
			/>
			<Pagination
				offset={data.transactions.offset}
				limit={data.transactions.limit}
				totalCount={data.transactions.totalCount}
				onchange={onPageChange}
			/>
		</Card>
	{/if}
</div>
