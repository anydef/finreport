<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import Card from '$lib/components/Card.svelte';
	import Field from '$lib/components/Field.svelte';
	import TransactionTable from '$lib/components/TransactionTable.svelte';
	import Pagination from '$lib/components/Pagination.svelte';
	import CategoryFilter from '$lib/components/CategoryFilter.svelte';
	import { PERIOD_PRESETS, type PeriodPresetId } from '$lib/period';
	import type { PageData } from './$types';

	let { data }: { data: PageData } = $props();

	let preset = $state(data.preset);
	let accountId = $state(data.accountId ?? '');
	let search = $state(data.search ?? '');
	let categorySlugs = $state(data.categorySlugs);
	let uncategorized = $state(data.uncategorized);
	let needsReview = $state(data.needsReview);
	let selectedTags = $state(data.tags);
	// Tri-state as a string so a <select> can represent "either" alongside true/false.
	let recurringFilter = $state(
		data.recurring === true ? 'true' : data.recurring === false ? 'false' : ''
	);
	let transferFilter = $state(
		data.transfer === true ? 'true' : data.transfer === false ? 'false' : ''
	);

	$effect(() => {
		preset = data.preset;
		accountId = data.accountId ?? '';
		search = data.search ?? '';
		categorySlugs = data.categorySlugs;
		uncategorized = data.uncategorized;
		needsReview = data.needsReview;
		selectedTags = data.tags;
		recurringFilter = data.recurring === true ? 'true' : data.recurring === false ? 'false' : '';
		transferFilter = data.transfer === true ? 'true' : data.transfer === false ? 'false' : '';
	});

	function applyFilters() {
		const params = new URLSearchParams();
		if (preset !== 'this-month') params.set('preset', preset as PeriodPresetId);
		if (accountId) params.set('accountId', accountId);
		if (search) params.set('search', search);
		if (categorySlugs.length) params.set('categorySlugs', categorySlugs.join(','));
		if (uncategorized) params.set('uncategorized', 'true');
		if (needsReview) params.set('needsReview', 'true');
		if (selectedTags.length) params.set('tags', selectedTags.join(','));
		if (recurringFilter) params.set('recurring', recurringFilter);
		if (transferFilter) params.set('transfer', transferFilter);
		goto(`${page.url.pathname}?${params.toString()}`, { keepFocus: true, noScroll: true });
	}

	function toggleTag(tag: string) {
		selectedTags = selectedTags.includes(tag)
			? selectedTags.filter((t) => t !== tag)
			: [...selectedTags, tag];
		applyFilters();
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

	<Card title="Filter by category">
		<CategoryFilter
			categories={data.categories}
			bind:selectedSlugs={categorySlugs}
			bind:uncategorized
			bind:needsReview
			onchange={applyFilters}
		/>
	</Card>

	<Card title="Filter by tag, recurring, transfer">
		<div class="flex flex-wrap items-start gap-6">
			<div class="flex flex-col gap-2">
				<p class="text-xs font-medium text-slate-500">Tags</p>
				{#if data.allTags.length === 0}
					<p class="text-sm text-slate-400">No tags yet.</p>
				{:else}
					<div class="flex flex-wrap gap-2">
						{#each data.allTags as tagCount (tagCount.tag)}
							<label class="flex items-center gap-1 text-sm">
								<input
									type="checkbox"
									checked={selectedTags.includes(tagCount.tag)}
									onchange={() => toggleTag(tagCount.tag)}
								/>
								{tagCount.tag} ({tagCount.transactionCount})
							</label>
						{/each}
					</div>
				{/if}
			</div>
			<Field label="Recurring" for="tx-recurring">
				<select
					id="tx-recurring"
					bind:value={recurringFilter}
					onchange={applyFilters}
					class="rounded-md border-slate-300 text-sm"
				>
					<option value="">Any</option>
					<option value="true">Recurring only</option>
					<option value="false">Non-recurring only</option>
				</select>
			</Field>
			<Field label="Transfer" for="tx-transfer">
				<select
					id="tx-transfer"
					bind:value={transferFilter}
					onchange={applyFilters}
					class="rounded-md border-slate-300 text-sm"
				>
					<option value="">Any</option>
					<option value="true">Transfers only</option>
					<option value="false">Non-transfers only</option>
				</select>
			</Field>
		</div>
	</Card>

	{#if data.error}
		<p role="alert" class="text-sm text-[var(--color-spending)]">
			Failed to load transactions from the GraphQL API.
		</p>
	{:else if data.transactions}
		<Card>
			<TransactionTable
				transactions={data.transactions.items}
				filter={data.transactionFilter}
				totalCount={data.transactions.totalCount}
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
