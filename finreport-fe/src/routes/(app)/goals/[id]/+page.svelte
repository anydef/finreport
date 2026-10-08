<script lang="ts">
	import { goto, invalidateAll } from '$app/navigation';
	import { page } from '$app/state';
	import { browser } from '$app/environment';
	import { createGraphqlClient } from '$lib/graphqlClient';
	import { ARCHIVE_GOAL_MUTATION, UPDATE_GOAL_MUTATION } from '$lib/graphql/queries';
	import type { GoalInput, TransactionSort } from '$lib/graphql/types';
	import { writeSort } from '$lib/transactionSort';
	import Button from '$lib/components/Button.svelte';
	import Card from '$lib/components/Card.svelte';
	import GoalBarChart from '$lib/components/GoalBarChart.svelte';
	import GoalCumulativeChart from '$lib/components/GoalCumulativeChart.svelte';
	import GoalForm from '$lib/components/GoalForm.svelte';
	import GoalProgressBar from '$lib/components/GoalProgressBar.svelte';
	import Modal from '$lib/components/Modal.svelte';
	import Pagination from '$lib/components/Pagination.svelte';
	import TransactionTable from '$lib/components/TransactionTable.svelte';
	import {
		bucketRangeParams,
		bucketTone,
		describeGoalPeriod,
		formatMoney,
		goalTotals,
		goalTypeLabel,
		periodUnit,
		progressFraction,
		selectedBucket,
		shapeCumulativeSeries,
		shapeGoalBars,
		toneText,
		toneTextClass,
		type GoalBarDatum
	} from '$lib/goalsView';
	import type { PageData } from './$types';

	let { data }: { data: PageData } = $props();

	let editing = $state(false);
	let busy = $state(false);
	let formError = $state('');

	const progress = $derived(data.error ? undefined : data.progress);
	const goal = $derived(progress?.goal);
	const totals = $derived(progress ? goalTotals(progress) : undefined);
	const bars = $derived(progress ? shapeGoalBars(progress.goal, progress.buckets) : []);
	const recurring = $derived(goal?.periodKind === 'RECURRING');
	const narrowed = $derived(
		!data.error && (data.txStart !== data.range.start || data.txEnd !== data.range.end)
	);
	const activeBucket = $derived(
		progress && !data.error ? selectedBucket(progress.buckets, data.txStart, data.txEnd) : null
	);
	const cumulative = $derived(
		progress && !data.error && !recurring
			? shapeCumulativeSeries(
					progress.goal,
					data.seriesTransactions?.items ?? [],
					data.range.start,
					data.range.end
				)
			: []
	);

	function navigate(
		opts: { txStart?: string; txEnd?: string; offset?: number; sort?: TransactionSort } = {}
	) {
		let params = new URLSearchParams();
		if (opts.txStart && opts.txEnd) {
			params.set('txStart', opts.txStart);
			params.set('txEnd', opts.txEnd);
		}
		// Before the offset: `writeSort` drops paging, so a new sort restarts at page 1.
		params = writeSort(opts.sort ?? data.sort, params);
		if (opts.offset) params.set('offset', String(opts.offset));
		const query = params.toString();
		goto(query ? `${page.url.pathname}?${query}` : page.url.pathname, {
			keepFocus: true,
			noScroll: true
		});
	}

	function selectBucket(bucket: { start: string; end: string }) {
		navigate(bucketRangeParams(bucket));
	}

	function onBarClick(bar: GoalBarDatum) {
		selectBucket(bar);
	}

	async function mutate(document: string, variables: Record<string, unknown>) {
		busy = true;
		formError = '';
		try {
			const result = await createGraphqlClient(fetch).mutation(document, variables).toPromise();
			if (result.error) throw new Error(result.error.message);
			return true;
		} catch (err) {
			formError = err instanceof Error ? err.message : 'Request failed';
			return false;
		} finally {
			busy = false;
		}
	}

	async function saveGoal(input: GoalInput) {
		if (await mutate(UPDATE_GOAL_MUTATION, { id: data.id, input })) {
			editing = false;
			await invalidateAll();
		}
	}

	async function archiveGoal() {
		if (await mutate(ARCHIVE_GOAL_MUTATION, { id: data.id })) await goto('/goals');
	}

	function closeForm() {
		editing = false;
		formError = '';
	}
</script>

<svelte:head>
	<title>{goal ? `${goal.name} · Goals` : 'Goal'} · finreport</title>
</svelte:head>

<div class="flex flex-col gap-6">
	<a href="/goals" class="text-sm text-slate-500 hover:text-slate-800">← All goals</a>

	{#if data.error || !progress || !goal || !totals}
		<p role="alert" class="text-sm text-[var(--color-spending)]">
			Failed to load this goal from the GraphQL API.
		</p>
	{:else}
		<div class="flex flex-wrap items-start justify-between gap-3">
			<div>
				<h1 class="text-lg font-semibold text-slate-900">{goal.name}</h1>
				<p class="text-sm text-slate-500">
					{goalTypeLabel(goal.type)} · {formatMoney(goal.amount, goal.currency)}
					{describeGoalPeriod(goal)}
				</p>
				<p class="text-xs text-slate-400">
					{[
						...goal.scope.categories.map((c) => c.name),
						...goal.scope.tags.map((t) => `#${t}`)
					].join(', ')}
				</p>
			</div>
			<div class="flex gap-2">
				<Button variant="secondary" onclick={() => (editing = true)}>Edit</Button>
				<Button variant="ghost" onclick={archiveGoal} disabled={busy}>Archive</Button>
			</div>
		</div>

		<div class="grid grid-cols-2 gap-4 lg:grid-cols-4">
			<Card>
				<p class="text-sm text-slate-500">{totals.totalLabel}</p>
				<p class="text-xl font-semibold text-slate-900">
					{formatMoney(totals.total, progress.currency)}
				</p>
			</Card>
			<Card>
				<p class="text-sm text-slate-500">{totals.remainingLabel}</p>
				<p class="text-xl font-semibold text-slate-900">
					{formatMoney(totals.remaining, progress.currency)}
				</p>
			</Card>
			<Card>
				<p class="text-sm text-slate-500">Average per {periodUnit(goal)}</p>
				<p class="text-xl font-semibold text-slate-900">
					{totals.average === null ? '—' : formatMoney(totals.average, progress.currency)}
				</p>
				{#if totals.average !== null}
					<p class="text-xs text-slate-400">Completed periods only</p>
				{/if}
			</Card>
			<Card>
				<p class="text-sm text-slate-500">Pending review</p>
				<p class="text-xl font-semibold text-slate-900">
					{formatMoney(totals.pending, progress.currency)}
				</p>
				<p class="text-xs text-slate-400">Held for review, not in the total</p>
			</Card>
		</div>

		<Card title={recurring ? 'Per period' : 'Cumulative'}>
			{#if browser}
				{#if recurring}
					<GoalBarChart
						{bars}
						threshold={goal.amount}
						currency={progress.currency}
						goalName={goal.name}
						{onBarClick}
					/>
				{:else}
					<GoalCumulativeChart
						points={cumulative}
						currency={progress.currency}
						goalName={goal.name}
						rangeEnd={data.range.end}
					/>
				{/if}
			{:else}
				<p class="flex h-72 items-center justify-center text-sm text-slate-400">Loading chart…</p>
			{/if}
			{#if !recurring}
				<p class="mt-2 text-xs text-slate-400">
					The line accumulates the matching transactions below; the figures above come from the
					server.
				</p>
			{/if}
		</Card>

		{#if recurring}
			<Card title="Periods">
				<ul class="flex flex-col divide-y divide-slate-100">
					{#each progress.buckets as bucket (bucket.start)}
						{@const tone = bucketTone(goal.type, goal.amount, bucket)}
						<li>
							<button
								type="button"
								onclick={() => selectBucket(bucket)}
								aria-pressed={activeBucket?.start === bucket.start}
								class="focus-visible:outline-brand grid w-full grid-cols-[6rem_1fr_9rem_8rem] items-center gap-3 rounded-md px-2 py-2 text-left text-sm hover:bg-slate-50 focus-visible:outline focus-visible:outline-2 aria-pressed:bg-slate-100"
							>
								<span class="font-medium text-slate-700">{bucket.label}</span>
								<GoalProgressBar
									fraction={progressFraction(bucket.total, goal.amount)}
									{tone}
									label="{bucket.label} progress"
								/>
								<span class="text-right text-slate-700">
									{formatMoney(bucket.total, progress.currency)}
									{#if bucket.pending !== '0.0000' && bucket.pending !== '0.00'}
										<span class="block text-xs text-slate-400">
											+{formatMoney(bucket.pending, progress.currency)} pending
										</span>
									{/if}
								</span>
								<span class="text-xs font-medium {toneTextClass(tone)}">
									{toneText(goal.type, tone, bucket.inProgress)}
								</span>
							</button>
						</li>
					{/each}
				</ul>
			</Card>
		{/if}

		<Card title="Matching transactions">
			{#if narrowed}
				<div class="mb-3 flex items-center justify-between">
					<p class="text-sm text-slate-500">
						{activeBucket ? activeBucket.label : `${data.txStart} to ${data.txEnd}`}
					</p>
					<Button variant="ghost" onclick={() => navigate()}>Clear filter</Button>
				</div>
			{/if}
			{#if data.transactionsError}
				<p role="alert" class="text-sm text-[var(--color-spending)]">
					Failed to load the matching transactions.
				</p>
			{:else if data.transactions}
				<TransactionTable
					transactions={data.transactions.items}
					currency={progress.currency}
					sort={data.sort}
					onsort={(next) => navigate({ txStart: data.txStart, txEnd: data.txEnd, sort: next })}
				/>
				<Pagination
					offset={data.transactions.offset}
					limit={data.transactions.limit}
					totalCount={data.transactions.totalCount}
					onchange={(offset) => navigate({ txStart: data.txStart, txEnd: data.txEnd, offset })}
				/>
			{/if}
		</Card>
	{/if}
</div>

{#if editing && goal}
	<Modal title="Edit goal" onclose={closeForm}>
		<GoalForm
			categories={data.categories}
			{goal}
			{busy}
			error={formError}
			onsubmit={saveGoal}
			oncancel={closeForm}
		/>
	</Modal>
{/if}
