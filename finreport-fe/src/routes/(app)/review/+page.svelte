<script lang="ts">
	import { goto, invalidateAll } from '$app/navigation';
	import { untrack } from 'svelte';
	import {
		EMPTY_SELECTION,
		describeResult,
		filterKey,
		headerState,
		isSelected,
		pruneSelection,
		reviewQueueFilter,
		selectedCount,
		selectionFilter,
		similarTarget,
		splitCount,
		toggleHeader,
		toggleRow,
		type Selection
	} from '$lib/bulkSelection';
	import { createGraphqlClient } from '$lib/graphqlClient';
	import {
		ENSURE_CATEGORY_MUTATION,
		SET_RULE_STATE_MUTATION,
		SET_TRANSACTION_CATEGORY_MUTATION,
		SPLIT_TRANSACTION_MUTATION,
		UNSPLIT_TRANSACTION_MUTATION
	} from '$lib/graphql/adminReviewQueries';
	import {
		SET_TRANSACTIONS_CATEGORY_MUTATION,
		SET_TRANSACTIONS_TAGS_MUTATION
	} from '$lib/graphql/queries';
	import { groupFilter, needsCreating, proposedCategoryInput, reviewHref } from '$lib/heldGroups';
	import type { BulkEditResult, HeldMerchantGroup, Rule, Transaction } from '$lib/graphql/types';
	import BulkEditDialog from '$lib/components/BulkEditDialog.svelte';
	import Card from '$lib/components/Card.svelte';
	import HeldGroupCard from '$lib/components/HeldGroupCard.svelte';
	import ReviewSelectionBar from '$lib/components/ReviewSelectionBar.svelte';
	import ReviewCard from '$lib/components/ReviewCard.svelte';
	import SplitEditor from '$lib/components/SplitEditor.svelte';
	import type { PageData } from './$types';

	let { data }: { data: PageData } = $props();

	let splitEditorTransaction = $state<Transaction | null>(null);
	let ruleBusyId = $state<string | null>(null);
	let ruleError = $state('');

	// --- Selection -------------------------------------------------------
	// The queue is paged, so "all" means every held transaction (the query's
	// totalCount), expressed as a filter, never the ids on screen. Every bulk
	// write is `needsReview: true` plus the selection, so it cannot reach a
	// transaction outside the queue.
	let selection = $state<Selection>(EMPTY_SELECTION);
	let bulkKind = $state<'category' | 'tags' | null>(null);

	const rows = $derived(data.reviewQueue?.transactions ?? []);
	const totalCount = $derived(data.reviewQueue?.totalCount ?? 0);
	const baseFilter = reviewQueueFilter();

	// --- Grouped view ----------------------------------------------------
	// A group is assigned with `{ needsReview: true, counterpartyKeys: [key] }`,
	// the same filter the flat view's bulk bar uses, so it cannot reach anything
	// outside the queue. The outcome is shown as the server reported it, and the
	// page reloads rather than hiding the group optimistically.
	let groupDialog = $state<HeldMerchantGroup | null>(null);
	let groupBusyKey = $state<string | null>(null);
	let notice = $state<{ tone: 'success' | 'partial' | 'failed'; text: string } | null>(null);

	const dialogGroupResult = (group: HeldMerchantGroup, slug: string) =>
		bulkMutate(
			SET_TRANSACTIONS_CATEGORY_MUTATION,
			{ filter: groupFilter(group), categorySlug: slug },
			'setTransactionsCategory'
		);

	async function groupDialogDone() {
		groupDialog = null;
		await invalidateAll();
	}

	/** Accept the LLM's unanimous proposal for the whole group, creating the category if it is new. */
	async function acceptProposal(group: HeldMerchantGroup, path: string) {
		groupBusyKey = group.counterpartyKey;
		notice = null;
		try {
			if (needsCreating(path, data.categories)) {
				await mutate(ENSURE_CATEGORY_MUTATION, { input: proposedCategoryInput(path) });
			}
			const result = await dialogGroupResult(group, path);
			const outcome = describeResult(result);
			notice = { tone: outcome.tone, text: `${group.displayName}: ${outcome.headline}` };
			await invalidateAll();
		} catch (err) {
			notice = {
				tone: 'failed',
				text: `${group.displayName}: ${err instanceof Error ? err.message : 'Could not apply the suggestion.'}`
			};
		} finally {
			groupBusyKey = null;
		}
	}
	// A selection belongs to the filter and page it was made under.
	const scopeKey = $derived(`${filterKey(baseFilter)}@${data.offset}`);
	$effect(() => {
		void scopeKey;
		untrack(() => {
			selection = EMPTY_SELECTION;
			bulkKind = null;
		});
	});
	// A per-card action resolves (removes) a card: drop it from the selection.
	$effect(() => {
		const ids = rows.map((r) => r.id);
		untrack(() => (selection = pruneSelection(selection, ids)));
	});

	const count = $derived(selectedCount(selection, totalCount));
	const header = $derived(headerState(selection));
	const similar = $derived(similarTarget(selection, rows));
	let headerBox = $state<HTMLInputElement | null>(null);
	$effect(() => {
		if (headerBox) headerBox.indeterminate = header === 'some';
	});

	const dialogNote =
		'These transactions are held for review. Assigning a category also resolves them: they leave this queue.';

	function applyBulk(field: string, query: string, variables: Record<string, unknown>) {
		return bulkMutate(
			query,
			{ filter: selectionFilter(baseFilter, selection), ...variables },
			field
		);
	}

	async function bulkMutate(
		query: string,
		variables: Record<string, unknown>,
		field: string
	): Promise<BulkEditResult> {
		const result = await mutate<Record<string, BulkEditResult | null>>(query, variables);
		const payload = result?.[field];
		if (!payload) throw new Error('The bulk edit could not be applied.');
		return payload;
	}

	/** A bulk edit resolves rows beyond this page: reload rather than patch. */
	async function bulkDone() {
		bulkKind = null;
		selection = EMPTY_SELECTION;
		await invalidateAll();
	}

	/**
	 * "Find similar" opens the ticked transaction's merchant group in the
	 * grouped view: the group is the same set of held transactions, with the
	 * assign-to-all action next to it.
	 */
	function findSimilar() {
		if (similar.kind !== 'ready') return;
		void goto(reviewHref('groups', similar.counterpartyKey));
	}

	function pageHref(offset: number): string {
		return offset > 0 ? `?view=flat&offset=${offset}` : reviewHref('flat');
	}

	function client() {
		return createGraphqlClient(fetch);
	}

	async function mutate<T>(document: string, variables: Record<string, unknown>): Promise<T> {
		const result = await client().mutation(document, variables).toPromise();
		if (result.error) throw new Error(result.error.message);
		return result.data as T;
	}

	async function setCategory(transactionId: string, categorySlug: string) {
		await mutate(SET_TRANSACTION_CATEGORY_MUTATION, { transactionId, categorySlug });
		await invalidateAll();
	}

	/**
	 * "Approve & create" for a `NEW_CATEGORY` suggestion (§6): creates the
	 * proposed category — parented under its first path segment when the
	 * path is nested (`housing.coworking` → parent `housing`) — then applies
	 * it to the transaction. Defaults the new category's `kind` to `EXPENSE`
	 * (the SDL has nothing richer to infer it from at review time); it can be
	 * corrected afterwards from `/admin/categories`.
	 */
	async function createAndApplyProposed(transactionId: string, proposedPath: string) {
		// `ensureCategory`, not `createCategory`: the LLM proposes the same
		// missing category for many held transactions, so the second approval
		// must reuse the category the first one created rather than failing
		// with "already exists".
		await mutate(ENSURE_CATEGORY_MUTATION, { input: proposedCategoryInput(proposedPath) });
		await setCategory(transactionId, proposedPath);
	}

	async function submitSplit(
		transactionId: string,
		parts: { amount: string; categorySlug: string }[]
	) {
		await mutate(SPLIT_TRANSACTION_MUTATION, { transactionId, parts });
		await invalidateAll();
	}

	async function unsplit(transactionId: string) {
		await mutate(UNSPLIT_TRANSACTION_MUTATION, { transactionId });
		await invalidateAll();
	}

	async function setRuleState(rule: Rule, state: 'ACTIVE' | 'REJECTED') {
		ruleBusyId = rule.id;
		ruleError = '';
		try {
			await mutate(SET_RULE_STATE_MUTATION, { id: rule.id, state });
			await invalidateAll();
		} catch (err) {
			ruleError = err instanceof Error ? err.message : 'Failed to update rule';
		} finally {
			ruleBusyId = null;
		}
	}
</script>

<svelte:head>
	<title>Review queue · finreport</title>
</svelte:head>

<div class="flex flex-col gap-6">
	{#if data.error}
		<p role="alert" class="text-sm text-[var(--color-spending)]">
			Failed to load the review queue from the GraphQL API.
		</p>
	{:else if data.reviewQueue}
		<Card title={`Held transactions (${totalCount})`}>
			<nav aria-label="Queue view" class="mb-3 flex items-center gap-2 text-xs">
				{#each [{ view: 'groups', label: 'By merchant' }, { view: 'flat', label: 'All transactions' }] as option (option.view)}
					<a
						href={reviewHref(option.view as 'groups' | 'flat')}
						aria-current={data.view === option.view ? 'page' : undefined}
						class="focus-visible:outline-brand cursor-pointer rounded-md px-2 py-1 font-medium focus-visible:outline focus-visible:outline-2 {data.view ===
						option.view
							? 'bg-brand text-white'
							: 'bg-slate-100 text-slate-700 hover:bg-slate-200'}"
					>
						{option.label}
					</a>
				{/each}
				{#if data.view === 'groups' && data.groups}
					<span class="text-slate-500">{data.groups.groupCount} merchants</span>
				{/if}
			</nav>
			{#if notice}
				<p
					role={notice.tone === 'success' ? 'status' : 'alert'}
					data-testid="group-notice"
					class="mb-3 rounded-md border p-2 text-xs {notice.tone === 'success'
						? 'border-emerald-300 bg-emerald-50 text-emerald-900'
						: 'border-amber-400 bg-amber-50 text-amber-900'}"
				>
					{notice.text}
				</p>
			{/if}
			{#if data.view === 'groups'}
				{#if !data.groups || data.groups.groups.length === 0}
					<p class="text-sm text-slate-500">Nothing needs review right now.</p>
				{:else}
					<div class="flex flex-col gap-3">
						{#each data.groups.groups as group (group.counterpartyKey ?? '\u0000ungrouped')}
							{@const open =
								group.counterpartyKey !== null && group.counterpartyKey === data.expand}
							<HeldGroupCard
								{group}
								expanded={open}
								toggleHref={reviewHref('groups', open ? null : group.counterpartyKey)}
								flatHref={reviewHref('flat')}
								busy={groupBusyKey === group.counterpartyKey}
								onAssign={() => (groupDialog = group)}
								onAccept={(path) => acceptProposal(group, path)}
							>
								{#each data.expandedTransactions as transaction (transaction.id)}
									<ReviewCard
										{transaction}
										categories={data.categories}
										onSetCategory={(slug) => setCategory(transaction.id, slug)}
										onCreateAndApplyProposed={(path) =>
											createAndApplyProposed(transaction.id, path)}
										onOpenSplitEditor={() => (splitEditorTransaction = transaction)}
									/>
								{/each}
							</HeldGroupCard>
						{/each}
					</div>
				{/if}
			{:else if rows.length === 0}
				<p class="text-sm text-slate-500">Nothing needs review right now.</p>
			{:else}
				<div class="flex flex-col gap-3">
					<label class="flex cursor-pointer items-center gap-2 text-xs text-slate-700">
						<input
							type="checkbox"
							bind:this={headerBox}
							checked={header === 'all'}
							onchange={() => (selection = toggleHeader(selection))}
							class="focus-visible:outline-brand h-4 w-4 cursor-pointer rounded border-slate-300 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2"
						/>
						Select all {totalCount} held transaction{totalCount === 1 ? '' : 's'}
						{#if totalCount > rows.length}
							<span class="text-slate-500">(showing {rows.length} on this page)</span>
						{/if}
					</label>
					{#if count > 0}
						<ReviewSelectionBar
							{count}
							allMatching={selection.mode === 'all-matching'}
							{similar}
							onAssignCategory={() => (bulkKind = 'category')}
							onAssignTags={() => (bulkKind = 'tags')}
							onFindSimilar={findSimilar}
							onClear={() => (selection = EMPTY_SELECTION)}
						/>
					{/if}
					{#each rows as transaction (transaction.id)}
						<ReviewCard
							{transaction}
							categories={data.categories}
							selected={isSelected(selection, transaction.id)}
							selectLocked={selection.mode === 'all-matching'}
							onselect={() => (selection = toggleRow(selection, transaction))}
							onSetCategory={(slug) => setCategory(transaction.id, slug)}
							onCreateAndApplyProposed={(path) => createAndApplyProposed(transaction.id, path)}
							onOpenSplitEditor={() => (splitEditorTransaction = transaction)}
						/>
					{/each}
					{#if data.offset > 0 || data.offset + rows.length < totalCount}
						<nav aria-label="Review queue pages" class="flex items-center gap-3 text-xs">
							{#if data.offset > 0}
								<a
									href={pageHref(Math.max(0, data.offset - data.limit))}
									class="focus-visible:outline-brand cursor-pointer rounded-md bg-slate-100 px-2 py-1 font-medium text-slate-700 hover:bg-slate-200 focus-visible:outline focus-visible:outline-2"
								>
									Previous
								</a>
							{/if}
							<span class="text-slate-500">
								{data.offset + 1}–{data.offset + rows.length} of {totalCount}
							</span>
							{#if data.offset + rows.length < totalCount}
								<a
									href={pageHref(data.offset + data.limit)}
									class="focus-visible:outline-brand cursor-pointer rounded-md bg-slate-100 px-2 py-1 font-medium text-slate-700 hover:bg-slate-200 focus-visible:outline focus-visible:outline-2"
								>
									Next
								</a>
							{/if}
						</nav>
					{/if}
				</div>
			{/if}
		</Card>

		<Card title={`Pending rules (${data.reviewQueue.pendingRules.length})`}>
			{#if ruleError}
				<p role="alert" class="mb-2 text-xs text-[var(--color-spending)]">{ruleError}</p>
			{/if}
			{#if data.reviewQueue.pendingRules.length === 0}
				<p class="text-sm text-slate-500">No learned rules awaiting approval.</p>
			{:else}
				<div class="flex flex-col gap-2">
					{#each data.reviewQueue.pendingRules as rule (rule.id)}
						<div
							class="flex flex-wrap items-center justify-between gap-2 rounded-md border border-slate-200 p-3"
						>
							<div>
								<p class="text-sm font-medium text-slate-900">{rule.name}</p>
								<p class="text-xs text-slate-500">
									→ {rule.category.name} · evidence {rule.evidenceCount}{rule.confidence != null
										? ` · confidence ${Math.round(rule.confidence * 100)}%`
										: ''}
								</p>
							</div>
							<div class="flex gap-2">
								<button
									type="button"
									disabled={ruleBusyId === rule.id}
									onclick={() => setRuleState(rule, 'ACTIVE')}
									class="focus-visible:outline-brand bg-brand hover:bg-brand/90 rounded-md px-2 py-1 text-xs font-medium text-white focus-visible:outline focus-visible:outline-2 disabled:opacity-50"
								>
									Approve
								</button>
								<button
									type="button"
									disabled={ruleBusyId === rule.id}
									onclick={() => setRuleState(rule, 'REJECTED')}
									class="focus-visible:outline-brand rounded-md bg-slate-100 px-2 py-1 text-xs font-medium text-slate-700 hover:bg-slate-200 focus-visible:outline focus-visible:outline-2 disabled:opacity-50"
								>
									Reject
								</button>
								<a
									href="/admin/rules"
									class="focus-visible:outline-brand rounded-md px-2 py-1 text-xs font-medium text-slate-600 hover:bg-slate-100 focus-visible:outline focus-visible:outline-2"
								>
									Edit in Rules
								</a>
							</div>
						</div>
					{/each}
				</div>
			{/if}
		</Card>
	{/if}
</div>

{#if groupDialog}
	{@const group = groupDialog}
	<BulkEditDialog
		kind="category"
		count={group.heldCount}
		splits={{ count: 0, exact: false }}
		categories={data.categories}
		note={`All ${group.heldCount} held transaction${group.heldCount === 1 ? '' : 's'} from ${group.displayName}. ${dialogNote}`}
		onApplyCategory={(slug) => dialogGroupResult(group, slug)}
		onApplyTags={() => Promise.reject(new Error('Tags are not edited per merchant group.'))}
		ondone={groupDialogDone}
		onclose={() => (groupDialog = null)}
	/>
{/if}

{#if bulkKind}
	<BulkEditDialog
		kind={bulkKind}
		{count}
		splits={splitCount(selection, rows)}
		categories={data.categories}
		note={bulkKind === 'category' ? dialogNote : undefined}
		onApplyCategory={(slug) =>
			applyBulk('setTransactionsCategory', SET_TRANSACTIONS_CATEGORY_MUTATION, {
				categorySlug: slug
			})}
		onApplyTags={(tags) =>
			applyBulk('setTransactionsTags', SET_TRANSACTIONS_TAGS_MUTATION, { tags })}
		ondone={bulkDone}
		onclose={() => (bulkKind = null)}
	/>
{/if}

{#if splitEditorTransaction}
	<SplitEditor
		transaction={splitEditorTransaction}
		categories={data.categories}
		onSubmit={(parts) => submitSplit(splitEditorTransaction!.id, parts)}
		onUnsplit={() => unsplit(splitEditorTransaction!.id)}
		onClose={() => (splitEditorTransaction = null)}
	/>
{/if}
