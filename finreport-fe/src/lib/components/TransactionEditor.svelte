<script lang="ts">
	/**
	 * The editor that expands below a transaction row (`TransactionItem` puts it
	 * in a full-width sub-row): the transaction's details with its classification editable in place:
	 * category (`setTransactionCategory`), tags (`setTransactionTags`, whole-set
	 * replace), the recurring flag (`setTransactionRecurring`) and a free-text
	 * note (`setTransactionNote`; commentary only, labelling never reads it).
	 * Category, tags and the flag are saved as they are made, the note on its
	 * Save button; the parent owns the mutations and hands back the
	 * updated transaction through the `transaction` prop. A rejected callback
	 * shows an error here and leaves the editor open. Splits are shown, not
	 * edited (SplitEditor is its own dialog). A category can be created in
	 * place (`CategoryCreate`); the result is handed up so the table's shared
	 * list, and so every other row's editor, has it too. Esc collapses
	 * (controls that use Esc themselves, like an open dropdown, consume it first).
	 */
	import CategoryCreate from './CategoryCreate.svelte';
	import Badge from './Badge.svelte';
	import SearchMenu from './SearchMenu.svelte';
	import TagEditor from './TagEditor.svelte';
	import RecurringBadge from './RecurringBadge.svelte';
	import { categoryOptionGroups } from '$lib/categoryTree';
	import { formatAmount, formatDisplayDate } from '$lib/format';
	import { rawCounterpartyName } from '$lib/displayNames';
	import { labelSourceBadge, needsReviewBadge } from '$lib/labelBadge';
	import { untrack } from 'svelte';
	import {
		categoryChangeWarning,
		isNoteDirty,
		MAX_NOTE_LENGTH,
		noteProblem,
		noteToSave
	} from '$lib/transactionEdit';
	import type { Category, Transaction } from '$lib/graphql/types';

	interface Props {
		transaction: Transaction;
		currency: string;
		/** `null` while still loading. */
		categories: Category[] | null;
		onSetCategory: (slug: string) => Promise<void>;
		onSetTags: (tags: string[]) => Promise<void>;
		onSetRecurring: (recurring: boolean | null) => Promise<void>;
		onSetNote: (note: string | null) => Promise<void>;
		/** A category was created in place; the owner adds it to the shared list. */
		onCategoryCreated: (category: Category) => void;
		onclose: () => void;
	}

	let {
		transaction: tx,
		currency,
		categories,
		onSetCategory,
		onSetTags,
		onSetRecurring,
		onSetNote,
		onCategoryCreated,
		onclose
	}: Props = $props();

	let busy = $state(false);
	let errorMessage = $state('');
	/** A category picked while splits exist, waiting for the user to confirm. */
	let pendingSlug = $state<string | null>(null);
	let creating = $state(false);

	/** The textarea's content; seeded once, then owned by the user until saved. */
	let noteDraft = $state(untrack(() => tx.note ?? ''));
	const noteDirty = $derived(isNoteDirty(tx.note, noteDraft));
	const noteIssue = $derived(noteProblem(noteDraft));

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

	async function categoryCreated(created: Category) {
		onCategoryCreated(created);
		creating = false;
		// Select it straight away; goes through the same split warning as any pick.
		pickCategory(created.slug);
	}

	function onKeydown(event: KeyboardEvent) {
		if (event.key === 'Escape' && !event.defaultPrevented) {
			event.stopPropagation();
			onclose();
		}
	}

	const saveTags = (tags: string[]) => run(() => onSetTags(tags), 'Failed to save tags');
	const saveNote = () =>
		run(async () => {
			await onSetNote(noteToSave(noteDraft));
			// Show what was stored (trimmed), so the button settles to "saved".
			noteDraft = noteToSave(noteDraft) ?? '';
		}, 'Failed to save the note');
	const saveRecurring = (next: boolean | null) =>
		run(() => onSetRecurring(next), 'Failed to update recurring flag');

	const btn =
		'focus-visible:outline-brand rounded-md px-3 py-1.5 text-xs font-medium focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 disabled:opacity-50';
</script>

<!-- Esc bubbles up from the controls inside; the ones that use it themselves stop it. -->
<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<section
	aria-label="Edit transaction"
	data-testid="transaction-editor"
	onkeydown={onKeydown}
	class="sticky left-0 box-border flex w-full max-w-[calc(100vw-2rem)] flex-col gap-4 py-3 pr-2 pl-2 lg:max-w-5xl"
>
	<div class="grid gap-x-8 gap-y-4 md:grid-cols-2">
		<div class="flex flex-col gap-4">
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
					<div class="flex flex-wrap items-center gap-2">
						<SearchMenu
							{groups}
							value={tx.label?.category?.slug ?? null}
							onchange={pickCategory}
							label="Category"
							placeholder={tx.label?.category?.name ?? 'Choose a category…'}
						/>
						{#if !creating}
							<button
								type="button"
								onclick={() => (creating = true)}
								class="{btn} bg-slate-100 text-slate-700 hover:bg-slate-200"
							>
								+ New category
							</button>
						{/if}
					</div>
					{#if creating}
						<CategoryCreate
							{categories}
							parentSlug={tx.label?.category?.slug.split('.')[0] ?? null}
							oncreated={categoryCreated}
							oncancel={() => (creating = false)}
						/>
					{/if}
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
					<div data-testid="split-parts" class="rounded-md bg-violet-50/60 p-2">
						<p class="mb-1 text-xs text-slate-500">
							Split into {tx.splits.length}
							{tx.splits.length === 1 ? 'part' : 'parts'} — totals count each part under its own category.
						</p>
						<ul class="flex flex-col gap-0.5" aria-label="Split parts">
							{#each tx.splits as part (part.index)}
								<li class="flex max-w-md items-center justify-between gap-4 text-sm">
									<span class="text-slate-700">{part.category.name}</span>
									<span class="font-medium whitespace-nowrap text-slate-700">
										{formatAmount(part.amount, currency)}
									</span>
								</li>
							{/each}
						</ul>
					</div>
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
		</div>

		<div class="flex flex-col gap-4">
			<dl class="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-sm">
				<dt class="text-slate-500">Booked</dt>
				<dd>{formatDisplayDate(tx.bookingDate)}</dd>
				{#if tx.valutaDate}
					<dt class="text-slate-500">Value date</dt>
					<dd>{formatDisplayDate(tx.valutaDate)}</dd>
				{/if}
				<dt class="text-slate-500">Status</dt>
				<dd>{tx.bookingStatus}</dd>
				{#if rawCounterpartyName(tx)}
					<!-- The nickname is shown in the row; the bank's own string stays visible as evidence. -->
					<dt class="text-slate-500">Bank name</dt>
					<dd class="break-words">{rawCounterpartyName(tx)}</dd>
				{/if}
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

			<section class="flex flex-col gap-2" aria-labelledby="tx-note-heading">
				<h3 id="tx-note-heading" class="text-xs font-medium text-slate-500">Note</h3>
				<textarea
					bind:value={noteDraft}
					rows="3"
					aria-labelledby="tx-note-heading"
					aria-invalid={noteIssue !== null}
					placeholder="Anything worth remembering about this transaction. Does not affect its category."
					class="focus-visible:outline-brand w-full rounded-md border border-slate-300 px-3 py-2 text-sm focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2"
				></textarea>
				<div class="flex items-center gap-2">
					<button
						type="button"
						disabled={busy || !noteDirty || noteIssue !== null}
						onclick={saveNote}
						class="{btn} bg-brand hover:bg-brand/90 text-white"
					>
						{noteToSave(noteDraft) === null && tx.note ? 'Clear note' : 'Save note'}
					</button>
					{#if noteIssue}
						<p role="alert" class="text-xs text-[var(--color-spending)]">{noteIssue}</p>
					{:else if !noteDirty && tx.note}
						<p class="text-xs text-slate-400">Saved</p>
					{:else}
						<p class="text-xs text-slate-400">{noteDraft.length}/{MAX_NOTE_LENGTH}</p>
					{/if}
				</div>
			</section>
		</div>
	</div>

	{#if errorMessage}
		<p role="alert" class="text-sm text-[var(--color-spending)]">{errorMessage}</p>
	{/if}

	<div class="flex justify-end">
		<button
			type="button"
			onclick={onclose}
			class="{btn} bg-slate-100 text-slate-700 hover:bg-slate-200"
		>
			Close
		</button>
	</div>
</section>
