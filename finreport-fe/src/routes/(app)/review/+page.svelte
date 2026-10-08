<script lang="ts">
	import { invalidateAll } from '$app/navigation';
	import { createGraphqlClient } from '$lib/graphqlClient';
	import {
		ENSURE_CATEGORY_MUTATION,
		SET_RULE_STATE_MUTATION,
		SET_TRANSACTION_CATEGORY_MUTATION,
		SPLIT_TRANSACTION_MUTATION,
		UNSPLIT_TRANSACTION_MUTATION
	} from '$lib/graphql/adminReviewQueries';
	import type { Rule, Transaction } from '$lib/graphql/types';
	import Card from '$lib/components/Card.svelte';
	import ReviewCard from '$lib/components/ReviewCard.svelte';
	import SplitEditor from '$lib/components/SplitEditor.svelte';
	import type { PageData } from './$types';

	let { data }: { data: PageData } = $props();

	let splitEditorTransaction = $state<Transaction | null>(null);
	let ruleBusyId = $state<string | null>(null);
	let ruleError = $state('');

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
		const segments = proposedPath.split('.');
		const slug = proposedPath;
		const name = segments[segments.length - 1]
			.split('_')
			.map((w) => w.charAt(0).toUpperCase() + w.slice(1))
			.join(' ');
		const parentSlug = segments.length > 1 ? segments.slice(0, -1).join('.') : undefined;
		// `ensureCategory`, not `createCategory`: the LLM proposes the same
		// missing category for many held transactions, so the second approval
		// must reuse the category the first one created rather than failing
		// with "already exists".
		await mutate(ENSURE_CATEGORY_MUTATION, {
			input: { slug, name, kind: 'EXPENSE', parentSlug }
		});
		await setCategory(transactionId, slug);
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
		<Card title={`Held transactions (${data.reviewQueue.transactions.length})`}>
			{#if data.reviewQueue.transactions.length === 0}
				<p class="text-sm text-slate-500">Nothing needs review right now.</p>
			{:else}
				<div class="flex flex-col gap-3">
					{#each data.reviewQueue.transactions as transaction (transaction.id)}
						<ReviewCard
							{transaction}
							categories={data.categories}
							onSetCategory={(slug) => setCategory(transaction.id, slug)}
							onCreateAndApplyProposed={(path) => createAndApplyProposed(transaction.id, path)}
							onOpenSplitEditor={() => (splitEditorTransaction = transaction)}
						/>
					{/each}
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

{#if splitEditorTransaction}
	<SplitEditor
		transaction={splitEditorTransaction}
		categories={data.categories}
		onSubmit={(parts) => submitSplit(splitEditorTransaction!.id, parts)}
		onUnsplit={() => unsplit(splitEditorTransaction!.id)}
		onClose={() => (splitEditorTransaction = null)}
	/>
{/if}
