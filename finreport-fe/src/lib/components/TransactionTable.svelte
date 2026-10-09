<script lang="ts">
	/**
	 * Transaction list. Rows are `TransactionItem`s; clicking one expands the
	 * editor (category, tags, the recurring flag, the note) in a sub-row below
	 * it. One row is expanded at a time (`expansion.ts`).
	 * The table owns the mutations: each response is folded into a local
	 * override of that row (see `transactionEdit.ts`), so the list shows the
	 * change at once without a refetch. Overrides are dropped whenever the
	 * parent hands in fresh `transactions`.
	 */
	import TransactionItem from './TransactionItem.svelte';
	import TransactionEditor from './TransactionEditor.svelte';
	import BulkEditDialog from './BulkEditDialog.svelte';
	import LinkDialog from './LinkDialog.svelte';
	import Badge from './Badge.svelte';
	import { invalidateAll } from '$app/navigation';
	import { untrack } from 'svelte';
	import { pruneExpanded, toggleExpanded } from '$lib/expansion';
	import { withCategory } from '$lib/categoryCreate';
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
		SET_TRANSACTION_NOTE_MUTATION,
		SET_TRANSACTION_RECURRING_MUTATION,
		SET_TRANSACTION_TAGS_MUTATION,
		SET_TRANSACTIONS_CATEGORY_MUTATION,
		SET_TRANSACTIONS_TAGS_MUTATION
	} from '$lib/graphql/queries';
	import {
		applyCategoryResult,
		applyNoteResult,
		applyRecurringResult,
		applyTagsResult
	} from '$lib/transactionEdit';
	import {
		dialogMemberFromLink,
		dialogMemberFromTransaction,
		linkHint,
		linkabilityOfSelection,
		statusLabel,
		type DialogMember
	} from '$lib/reimbursement';
	import type {
		BulkEditResult,
		Category,
		Transaction,
		TransactionFilter,
		TransactionLink,
		TransactionSort,
		TransactionSortField
	} from '$lib/graphql/types';

	interface Props {
		transactions: Transaction[];
		currency: string;
		/**
		 * The filter the rows were loaded with. Giving it (with `totalCount`)
		 * turns on multi-select and bulk edit; without it the table is read-only
		 * apart from the row editor.
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
	/** The reimbursement-link dialog, when open. */
	let linkDialog = $state<{
		anchor: DialogMember;
		initial: DialogMember[];
		link: TransactionLink | null;
	} | null>(null);

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

	// Linking works on ticked rows (not "all matching", which can span pages).
	const ticked = $derived(
		selection.mode === 'ids' ? rows.filter((r) => selection.mode === 'ids' && selection.ids.includes(r.id)) : []
	);
	const linkable = $derived(
		ticked.length >= 2 ? linkabilityOfSelection(ticked) : { ok: true as const, expenses: [], offsets: [] }
	);
	const linkBlocked = $derived(
		selection.mode !== 'ids'
			? 'Link works on individually ticked rows.'
			: ticked.length === 1 && ticked[0].link
				? 'This transaction is already linked. Use Edit link on its row.'
				: ticked.length >= 2 && !linkable.ok
					? linkable.reason
					: null
	);

	function openLinkFromSelection() {
		if (linkBlocked || ticked.length === 0) return;
		const members = ticked.map(dialogMemberFromTransaction);
		linkDialog = { anchor: members[0], initial: members, link: null };
	}

	function openLinkEditor(tx: Transaction) {
		const link = tx.link;
		if (!link) return;
		const members = link.members
			.map(dialogMemberFromLink)
			.filter((m): m is DialogMember => m !== null);
		const anchor = members.find((m) => m.id === tx.id) ?? members[0];
		if (!anchor) return;
		linkDialog = { anchor, initial: members, link };
	}

	async function linkDone() {
		linkDialog = null;
		selection = EMPTY_SELECTION;
		await invalidateAll();
	}

	const statusVariant = {
		FULL: 'success',
		PARTIAL: 'warning',
		OVER: 'info',
		INCOMPLETE: 'neutral'
	} as const;

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

	function toggle(tx: Transaction) {
		selectedId = toggleExpanded(selectedId, tx.id);
		if (selectedId) loadCategories();
	}

	// An expanded row that leaves the list (new page or filter) collapses.
	$effect(() => {
		const ids = transactions.map((tx) => tx.id);
		untrack(() => (selectedId = pruneExpanded(selectedId, ids)));
	});

	/** A category created in place joins the list every row's editor shares. */
	function categoryCreated(created: Category) {
		categories = withCategory(categories ?? [], created);
	}

	/** Run a mutation and return its payload, or throw so the editor can show it. */
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

	async function setNote(note: string | null) {
		if (!selected) return;
		const base = selected;
		const res = await mutate<{ note: string | null }>(
			SET_TRANSACTION_NOTE_MUTATION,
			{ transactionId: base.id, note },
			'setTransactionNote'
		);
		edited[base.id] = applyNoteResult(edited[base.id] ?? base, res);
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
									disabled={ticked.length === 0 || linkBlocked !== null}
									title={linkBlocked ?? 'Link a reimbursement to the expense it offsets'}
									onclick={openLinkFromSelection}
									class="{bulkBtn} bg-brand hover:bg-brand/90 text-white"
									data-testid="bulk-link"
								>
									{ticked.length === 1 ? 'Find reimbursement...' : 'Link as reimbursement'}
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
						expanded={tx.id === selectedId}
						ontoggle={toggle}
						onclose={() => (selectedId = null)}
						selected={isSelected(selection, tx.id)}
						selectLocked={selection.mode === 'all-matching'}
						onselect={bulkEnabled ? () => (selection = toggleRow(selection, tx)) : undefined}
					>
						{#snippet editor(close)}
							<TransactionEditor
								transaction={tx}
								{currency}
								{categories}
								onSetCategory={setCategory}
								onSetTags={setTags}
								onSetRecurring={setRecurring}
								onSetNote={setNote}
								onCategoryCreated={categoryCreated}
								onclose={close}
							/>
						{/snippet}
					</TransactionItem>
					{@const hint = linkHint(tx)}
					{#if hint && tx.link}
						<tr class="border-b border-slate-100 bg-slate-50/70" data-testid="link-hint">
							<td {colspan} class="py-1.5 pr-4 pl-3 text-xs text-slate-600">
								<span class="flex flex-wrap items-center gap-2">
									<span aria-hidden="true">↳</span>
									<Badge text={statusLabel(hint.status)} variant={statusVariant[hint.status]} />
									<span>{hint.text}</span>
									{#if tx.link.note}<span class="text-slate-500 italic">"{tx.link.note}"</span>{/if}
									<button
										type="button"
										onclick={() => openLinkEditor(tx)}
										class="text-brand focus-visible:outline-brand rounded px-1 underline hover:no-underline focus-visible:outline focus-visible:outline-2"
									>
										Edit link
									</button>
								</span>
							</td>
						</tr>
					{/if}
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

{#if linkDialog}
	<LinkDialog
		anchor={linkDialog.anchor}
		initial={linkDialog.initial}
		link={linkDialog.link}
		ondone={linkDone}
		onclose={() => (linkDialog = null)}
	/>
{/if}
