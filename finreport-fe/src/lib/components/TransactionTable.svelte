<script lang="ts">
	/**
	 * Transaction list. Rows are `TransactionItem`s; clicking one opens the
	 * detail modal where category, tags and the recurring flag are edited.
	 * The table owns the mutations: each response is folded into a local
	 * override of that row (see `transactionEdit.ts`), so the list shows the
	 * change at once without a refetch. Overrides are dropped whenever the
	 * parent hands in fresh `transactions`.
	 */
	import TransactionItem from './TransactionItem.svelte';
	import TransactionDetailModal from './TransactionDetailModal.svelte';
	import BulkEditDialog from './BulkEditDialog.svelte';
	import { invalidateAll } from '$app/navigation';
	import { untrack } from 'svelte';
	import {
		EMPTY_SELECTION,
		filterKey,
		headerState,
		isSelected,
		selectedCount,
		selectionFilter,
		splitCount,
		toggleHeader,
		toggleRow,
		type Selection
	} from '$lib/bulkSelection';
	import SortHeader from './SortHeader.svelte';
	import { DEFAULT_SORT, nextSort, sortKey } from '$lib/transactionSort';
	import { createGraphqlClient } from '$lib/graphqlClient';
	import {
		CATEGORIES_QUERY,
		SET_TRANSACTION_CATEGORY_MUTATION,
		SET_TRANSACTION_RECURRING_MUTATION,
		SET_TRANSACTION_TAGS_MUTATION,
		SET_TRANSACTIONS_CATEGORY_MUTATION,
		SET_TRANSACTIONS_TAGS_MUTATION
	} from '$lib/graphql/queries';
	import { applyCategoryResult, applyRecurringResult, applyTagsResult } from '$lib/transactionEdit';
	import type {
		BulkEditResult,
		Category,
		Transaction,
		TransactionFilter,
		TransactionSort,
		TransactionSortField
	} from '$lib/graphql/types';

	interface Props {
		transactions: Transaction[];
		currency: string;
		/**
		 * The filter the rows were loaded with. Giving it (with `totalCount`)
		 * turns on multi-select and bulk edit; without it the table is read-only
		 * apart from the detail modal.
		 */
		filter?: TransactionFilter;
		/** Rows matching `filter` across all pages, not just the loaded ones. */
		totalCount?: number;
		/**
		 * The server-side ordering the rows were loaded with. Giving `onsort`
		 * turns the Date, Counterparty, Category and Amount headers into sort
		 * buttons. The parent re-queries; the table never reorders rows itself,
		 * because it only holds one page of them.
		 */
		sort?: TransactionSort;
		onsort?: (next: TransactionSort) => void;
	}

	let {
		transactions,
		currency,
		filter,
		totalCount = 0,
		sort = DEFAULT_SORT,
		onsort
	}: Props = $props();

	let edited = $state<Record<string, Transaction>>({});
	let selectedId = $state<string | null>(null);
	let categories = $state<Category[] | null>(null);

	$effect(() => {
		void transactions;
		edited = {};
	});

	const rows = $derived(transactions.map((tx) => edited[tx.id] ?? tx));
	const selected = $derived(rows.find((tx) => tx.id === selectedId) ?? null);

	const bulkEnabled = $derived(filter !== undefined);
	let selection = $state<Selection>(EMPTY_SELECTION);
	let bulkKind = $state<'category' | 'tags' | null>(null);

	// A selection belongs to the filter and the ordering it was made under.
	// When either changes, drop it: a stale selection is how a bulk tool edits
	// wrong rows (an explicit id list would silently span a reordering).
	const selectionScope = $derived(`${filterKey(filter ?? {})}|${sortKey(sort)}`);
	$effect(() => {
		void selectionScope;
		untrack(() => {
			selection = EMPTY_SELECTION;
			bulkKind = null;
		});
	});

	const count = $derived(selectedCount(selection, totalCount));
	const header = $derived(headerState(selection));
	const multiSelect = $derived(bulkEnabled && count > 0);
	const colspan = $derived(bulkEnabled ? 8 : 7);
	let headerBox = $state<HTMLInputElement | null>(null);
	$effect(() => {
		if (headerBox) headerBox.indeterminate = header === 'some';
	});

	/** Headers are plain text unless the parent can re-query with a new sort. */
	const sortTo = $derived(
		onsort ? (field: TransactionSortField) => onsort(nextSort(sort, field)) : undefined
	);

	function openBulk(kind: 'category' | 'tags') {
		bulkKind = kind;
		loadCategories();
	}

	async function applyBulkCategory(slug: string) {
		return mutate<BulkEditResult>(
			SET_TRANSACTIONS_CATEGORY_MUTATION,
			{ filter: selectionFilter(filter ?? {}, selection), categorySlug: slug },
			'setTransactionsCategory'
		);
	}

	async function applyBulkTags(tags: string[]) {
		return mutate<BulkEditResult>(
			SET_TRANSACTIONS_TAGS_MUTATION,
			{ filter: selectionFilter(filter ?? {}, selection), tags },
			'setTransactionsTags'
		);
	}

	/** A bulk change can touch rows beyond this page, so reload rather than patch. */
	async function bulkDone() {
		bulkKind = null;
		selection = EMPTY_SELECTION;
		await invalidateAll();
	}

	const bulkBtn =
		'focus-visible:outline-brand rounded-md px-3 py-1 text-xs font-medium focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 disabled:opacity-50';

	const client = () => createGraphqlClient(fetch);

	async function loadCategories() {
		if (categories) return;
		const result = await client().query(CATEGORIES_QUERY, { includeArchived: false }).toPromise();
		categories = (result.data?.categories ?? []) as Category[];
	}

	function open(tx: Transaction) {
		selectedId = tx.id;
		loadCategories();
	}

	/** Run a mutation and return its payload, or throw so the modal can show it. */
	async function mutate<T>(
		query: string,
		variables: Record<string, unknown>,
		field: string
	): Promise<T> {
		const result = await client().mutation(query, variables).toPromise();
		if (result.error || !result.data?.[field]) {
			throw new Error(result.error?.message ?? 'The change could not be saved.');
		}
		return result.data[field] as T;
	}

	async function setCategory(slug: string) {
		if (!selected) return;
		const base = selected;
		const res = await mutate<Parameters<typeof applyCategoryResult>[1]>(
			SET_TRANSACTION_CATEGORY_MUTATION,
			{ transactionId: base.id, categorySlug: slug },
			'setTransactionCategory'
		);
		edited[base.id] = applyCategoryResult(edited[base.id] ?? base, res);
	}

	async function setTags(tags: string[]) {
		if (!selected) return;
		const base = selected;
		const res = await mutate<{ tags: string[] }>(
			SET_TRANSACTION_TAGS_MUTATION,
			{ transactionId: base.id, tags },
			'setTransactionTags'
		);
		edited[base.id] = applyTagsResult(edited[base.id] ?? base, res);
	}

	async function setRecurring(recurring: boolean | null) {
		if (!selected) return;
		const base = selected;
		const res = await mutate<Parameters<typeof applyRecurringResult>[1]>(
			SET_TRANSACTION_RECURRING_MUTATION,
			{ transactionId: base.id, recurring },
			'setTransactionRecurring'
		);
		edited[base.id] = applyRecurringResult(edited[base.id] ?? base, res);
	}
</script>

{#if rows.length === 0}
	<p class="py-8 text-center text-sm text-slate-500">No transactions in this period.</p>
{:else}
	<div class="overflow-x-auto">
		<table class="w-full text-sm">
			<thead>
				{#if multiSelect}
					<tr class="border-b border-slate-200 bg-slate-50" data-testid="bulk-bar">
						<th {colspan} class="py-2 pr-4 pl-1 text-left font-normal">
							<div class="flex flex-wrap items-center gap-2">
								<span role="status" class="text-sm font-medium text-slate-900">
									{selection.mode === 'all-matching'
										? `All ${count} matching selected`
										: `${count} selected`}
								</span>
								<button
									type="button"
									disabled={count === 0}
									onclick={() => openBulk('category')}
									class="{bulkBtn} bg-brand hover:bg-brand/90 text-white"
								>
									Edit categories
								</button>
								<button
									type="button"
									disabled={count === 0}
									onclick={() => openBulk('tags')}
									class="{bulkBtn} bg-brand hover:bg-brand/90 text-white"
								>
									Edit tags
								</button>
								<button
									type="button"
									onclick={() => (selection = EMPTY_SELECTION)}
									class="{bulkBtn} bg-slate-100 text-slate-700 hover:bg-slate-200"
								>
									Clear selection
								</button>
							</div>
						</th>
					</tr>
				{/if}
				<tr class="border-b border-slate-200 text-left text-slate-500">
					{#if bulkEnabled}
						<th class="py-2 pr-2 pl-1">
							<input
								bind:this={headerBox}
								type="checkbox"
								checked={header === 'all'}
								onchange={() => (selection = toggleHeader(selection))}
								aria-label="Select all {totalCount} matching transactions"
								class="focus-visible:outline-brand h-4 w-4 rounded border-slate-300 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2"
							/>
						</th>
					{/if}
					<SortHeader label="Date" field="BOOKING_DATE" {sort} onsort={sortTo} />
					<SortHeader label="Counterparty" field="COUNTERPARTY_NAME" {sort} onsort={sortTo} />
					<th class="py-2 pr-4 font-medium">Description</th>
					<SortHeader label="Category" field="CATEGORY" {sort} onsort={sortTo} />
					<th class="py-2 pr-4 font-medium">Tags</th>
					<th class="py-2 pr-4 font-medium">Flags</th>
					<SortHeader label="Amount" field="AMOUNT" {sort} onsort={sortTo} align="right" />
				</tr>
			</thead>
			<tbody>
				{#each rows as tx (tx.id)}
					<TransactionItem
						transaction={tx}
						{currency}
						onopen={open}
						selected={isSelected(selection, tx.id)}
						selectLocked={selection.mode === 'all-matching'}
						onselect={bulkEnabled ? () => (selection = toggleRow(selection, tx)) : undefined}
					/>
				{/each}
			</tbody>
		</table>
	</div>
{/if}

{#if bulkKind}
	<BulkEditDialog
		kind={bulkKind}
		{count}
		splits={splitCount(selection, rows)}
		{categories}
		onApplyCategory={applyBulkCategory}
		onApplyTags={applyBulkTags}
		ondone={bulkDone}
		onclose={() => (bulkKind = null)}
	/>
{/if}

{#if selected}
	<TransactionDetailModal
		transaction={selected}
		{currency}
		{categories}
		onSetCategory={setCategory}
		onSetTags={setTags}
		onSetRecurring={setRecurring}
		onclose={() => (selectedId = null)}
	/>
{/if}
