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
	import { createGraphqlClient } from '$lib/graphqlClient';
	import {
		CATEGORIES_QUERY,
		SET_TRANSACTION_CATEGORY_MUTATION,
		SET_TRANSACTION_RECURRING_MUTATION,
		SET_TRANSACTION_TAGS_MUTATION
	} from '$lib/graphql/queries';
	import { applyCategoryResult, applyRecurringResult, applyTagsResult } from '$lib/transactionEdit';
	import type { Category, Transaction } from '$lib/graphql/types';

	interface Props {
		transactions: Transaction[];
		currency: string;
	}

	let { transactions, currency }: Props = $props();

	let edited = $state<Record<string, Transaction>>({});
	let selectedId = $state<string | null>(null);
	let categories = $state<Category[] | null>(null);

	$effect(() => {
		void transactions;
		edited = {};
	});

	const rows = $derived(transactions.map((tx) => edited[tx.id] ?? tx));
	const selected = $derived(rows.find((tx) => tx.id === selectedId) ?? null);

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
				<tr class="border-b border-slate-200 text-left text-slate-500">
					<th class="py-2 pr-4 font-medium">Date</th>
					<th class="py-2 pr-4 font-medium">Counterparty</th>
					<th class="py-2 pr-4 font-medium">Description</th>
					<th class="py-2 pr-4 font-medium">Category</th>
					<th class="py-2 pr-4 font-medium">Tags</th>
					<th class="py-2 pr-4 font-medium">Flags</th>
					<th class="py-2 pr-4 text-right font-medium">Amount</th>
				</tr>
			</thead>
			<tbody>
				{#each rows as tx (tx.id)}
					<TransactionItem transaction={tx} {currency} onopen={open} />
				{/each}
			</tbody>
		</table>
	</div>
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
