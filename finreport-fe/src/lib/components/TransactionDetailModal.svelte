<script lang="ts">
	/**
	 * Detail view of one transaction with its classification editable in place:
	 * category (`setTransactionCategory`), tags (`setTransactionTags`, whole-set
	 * replace) and the recurring flag (`setTransactionRecurring`). Each edit is
	 * saved as it is made; the parent owns the mutations and hands back the
	 * updated transaction through the `transaction` prop. A rejected callback
	 * shows an error here and leaves the dialog open. Splits are shown, not
	 * edited (SplitEditor is its own dialog).
	 */
	import Modal from './Modal.svelte';
	import Badge from './Badge.svelte';
	import SearchMenu from './SearchMenu.svelte';
	import TagEditor from './TagEditor.svelte';
	import RecurringBadge from './RecurringBadge.svelte';
	import { categoryOptionGroups } from '$lib/categoryTree';
	import { formatAmount, formatDisplayDate } from '$lib/format';
	import { labelSourceBadge, needsReviewBadge } from '$lib/labelBadge';
	import { categoryChangeWarning } from '$lib/transactionEdit';
	import type { Category, Transaction } from '$lib/graphql/types';

	interface Props {
		transaction: Transaction;
		currency: string;
		/** `null` while still loading. */
		categories: Category[] | null;
		onSetCategory: (slug: string) => Promise<void>;
		onSetTags: (tags: string[]) => Promise<void>;
		onSetRecurring: (recurring: boolean | null) => Promise<void>;
		onclose: () => void;
	}

	let {
		transaction: tx,
		currency,
		categories,
		onSetCategory,
		onSetTags,
		onSetRecurring,
		onclose
	}: Props = $props();

	let busy = $state(false);
	let errorMessage = $state('');
	/** A category picked while splits exist, waiting for the user to confirm. */
	let pendingSlug = $state<string | null>(null);

	const groups = $derived(categoryOptionGroups(categories ?? []));
	const pendingLabel = $derived(
		groups.flatMap((g) => g.options).find((o) => o.slug === pendingSlug)?.label ?? pendingSlug
	);
	const warning = $derived(categoryChangeWarning(tx));
	const sourceBadge = $derived(labelSourceBadge(tx.label));
	const reviewPill = $derived(needsReviewBadge(tx.label));

	async function run(action: () => Promise<void>, fallback: string) {
		busy = true;
		errorMessage = '';
		try {
			await action();
		} catch (err) {
			errorMessage = err instanceof Error ? err.message : fallback;
		} finally {
			busy = false;
		}
	}

	function pickCategory(slug: string) {
		if (slug === tx.label?.category?.slug) return;
		if (warning) {
			pendingSlug = slug;
			return;
		}
		run(() => onSetCategory(slug), 'Failed to change category');
	}

	async function confirmCategory() {
		const slug = pendingSlug;
		if (!slug) return;
		pendingSlug = null;
		await run(() => onSetCategory(slug), 'Failed to change category');
	}

	const saveTags = (tags: string[]) => run(() => onSetTags(tags), 'Failed to save tags');
	const saveRecurring = (next: boolean | null) =>
		run(() => onSetRecurring(next), 'Failed to update recurring flag');

	const btn =
		'focus-visible:outline-brand rounded-md px-3 py-1.5 text-xs font-medium focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 disabled:opacity-50';
</script>

<Modal title="Transaction details" {onclose} sheet>
	<div class="flex items-start justify-between gap-4">
		<div class="min-w-0">
			<p class="truncate text-base font-semibold text-slate-900">
				{tx.counterpartyName ?? 'Unknown'}
			</p>
			{#if tx.description}
				<p class="text-sm break-words text-slate-600">{tx.description}</p>
			{/if}
		</div>
		<p
			class="text-base font-semibold whitespace-nowrap"
			class:text-[color:var(--color-income)]={Number(tx.amount) > 0}
			class:text-[color:var(--color-spending)]={Number(tx.amount) < 0}
		>
			{formatAmount(tx.amount, currency)}
		</p>
	</div>

	<dl class="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-sm">
		<dt class="text-slate-500">Booked</dt>
		<dd>{formatDisplayDate(tx.bookingDate)}</dd>
		{#if tx.valutaDate}
			<dt class="text-slate-500">Value date</dt>
			<dd>{formatDisplayDate(tx.valutaDate)}</dd>
		{/if}
		<dt class="text-slate-500">Status</dt>
		<dd>{tx.bookingStatus}</dd>
		{#if tx.counterpartyIban}
			<dt class="text-slate-500">IBAN</dt>
			<dd class="break-all">{tx.counterpartyIban}</dd>
		{/if}
		{#if tx.transfer}
			<dt class="text-slate-500">Transfer</dt>
			<dd class="flex flex-wrap items-center gap-1">
				<Badge text="⇄ transfer" variant="info" />
				<span class="text-slate-600">
					{tx.transfer.counterpartTransactionId
						? `matched (${tx.transfer.match.toLowerCase()}), other leg ${tx.transfer.counterpartTransactionId.slice(0, 8)}`
						: 'other leg not projected yet'}
				</span>
			</dd>
		{/if}
		{#if tx.recurring.isRecurring && tx.recurring.cadence}
			<dt class="text-slate-500">Series</dt>
			<dd>
				{tx.recurring.cadence.toLowerCase()}{#if tx.recurring.medianAmount}, about {formatAmount(
						tx.recurring.medianAmount,
						currency
					)}{/if}
				<a href="/recurring" class="text-[var(--color-brand)] hover:underline">view series</a>
			</dd>
		{/if}
	</dl>

	<hr class="border-slate-200" />

	<section class="flex flex-col gap-2" aria-labelledby="tx-category-heading">
		<div class="flex flex-wrap items-center gap-2">
			<h3 id="tx-category-heading" class="text-xs font-medium text-slate-500">Category</h3>
			{#if sourceBadge}
				<Badge text={sourceBadge.text} variant={sourceBadge.variant} />
			{/if}
			{#if reviewPill}
				<Badge text={reviewPill.text} variant={reviewPill.variant} />
			{/if}
		</div>
		{#if categories === null}
			<p class="text-sm text-slate-400">Loading categories…</p>
		{:else}
			<SearchMenu
				{groups}
				value={tx.label?.category?.slug ?? null}
				onchange={pickCategory}
				label="Category"
				placeholder={tx.label?.category?.name ?? 'Choose a category…'}
			/>
		{/if}
		{#if pendingSlug && warning}
			<div role="alert" class="rounded-md border border-amber-300 bg-amber-50 p-3 text-sm">
				<p class="text-amber-900">{warning}</p>
				<p class="mt-1 text-amber-900">New category: <strong>{pendingLabel}</strong></p>
				<div class="mt-2 flex gap-2">
					<button
						type="button"
						disabled={busy}
						onclick={confirmCategory}
						class="{btn} bg-amber-600 text-white hover:bg-amber-700"
					>
						Change category and remove split
					</button>
					<button
						type="button"
						onclick={() => (pendingSlug = null)}
						class="{btn} bg-slate-100 text-slate-700 hover:bg-slate-200"
					>
						Cancel
					</button>
				</div>
			</div>
		{/if}
		{#if tx.splits.length > 0}
			<ul class="text-sm text-slate-600" aria-label="Split parts">
				{#each tx.splits as part (part.index)}
					<li>{part.category.name}: {formatAmount(part.amount, currency)}</li>
				{/each}
			</ul>
		{/if}
	</section>

	<section class="flex flex-col gap-2" aria-labelledby="tx-tags-heading">
		<h3 id="tx-tags-heading" class="text-xs font-medium text-slate-500">Tags</h3>
		<TagEditor tags={tx.tags} onSave={saveTags} />
	</section>

	<section class="flex flex-col gap-2" aria-labelledby="tx-recurring-heading">
		<h3 id="tx-recurring-heading" class="text-xs font-medium text-slate-500">
			Recurring (click to cycle auto / yes / no)
		</h3>
		<div><RecurringBadge recurring={tx.recurring} onToggle={saveRecurring} /></div>
	</section>

	{#if errorMessage}
		<p role="alert" class="text-sm text-[var(--color-spending)]">{errorMessage}</p>
	{/if}

	<div class="mt-auto flex justify-end">
		<button
			type="button"
			onclick={onclose}
			class="{btn} bg-slate-100 text-slate-700 hover:bg-slate-200"
		>
			Done
		</button>
	</div>
</Modal>
