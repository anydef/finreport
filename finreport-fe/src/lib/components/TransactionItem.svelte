<script lang="ts">
	/**
	 * One transaction row, shared by every transaction table. The whole row is
	 * the click target (mouse); the counterparty cell holds a real button so
	 * keyboard and screen-reader users get the same action and focus has
	 * somewhere to return to when the detail modal closes. Links inside the
	 * row (the "needs review" pill) keep their own behaviour.
	 */
	import { formatAmount, formatDisplayDate } from '$lib/format';
	import { labelSourceBadge, needsReviewBadge } from '$lib/labelBadge';
	import { splitIndicator } from '$lib/splitView';
	import Badge from './Badge.svelte';
	import type { Transaction } from '$lib/graphql/types';

	interface Props {
		transaction: Transaction;
		currency: string;
		onopen: (transaction: Transaction) => void;
		/** Bulk-edit mode: render a checkbox cell. Omit `onselect` and there is none. */
		selected?: boolean;
		/** Ticked as part of "all matching": shown checked, not individually untickable. */
		selectLocked?: boolean;
		onselect?: () => void;
	}

	let {
		transaction: tx,
		currency,
		onopen,
		selected = false,
		selectLocked = false,
		onselect
	}: Props = $props();

	let opener = $state<HTMLButtonElement | null>(null);

	function onRowClick(event: MouseEvent) {
		if ((event.target as HTMLElement).closest('a, input, [data-select-cell], [data-split-toggle]'))
			return;
		// Focus first so the modal records this row's button as the element to return to.
		opener?.focus();
		onopen(tx);
	}

	const sourceBadge = $derived(labelSourceBadge(tx.label));
	const reviewPill = $derived(needsReviewBadge(tx.label));
	// A split's own label has no category (it lives on the parts), so the cell
	// shows the split pill instead of "—". Its label source is always "you", so
	// that badge is dropped for splits: one pill, not two, keeps the row calm.
	const split = $derived(splitIndicator(tx.splits));
	let partsOpen = $state(false);
</script>

<!-- The counterparty button below is the keyboard/AT equivalent of this row click. -->
<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
<tr
	class="cursor-pointer border-b border-slate-100 last:border-0 focus-within:bg-slate-50 hover:bg-slate-50"
	onclick={onRowClick}
>
	{#if onselect}
		<td class="py-2 pr-2 pl-1" data-select-cell>
			<input
				type="checkbox"
				checked={selected}
				disabled={selectLocked}
				onchange={onselect}
				aria-label="Select transaction {tx.counterpartyName ?? 'Unknown'} on {formatDisplayDate(
					tx.bookingDate
				)}"
				class="focus-visible:outline-brand h-4 w-4 rounded border-slate-300 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2"
			/>
		</td>
	{/if}
	<td class="py-2 pr-4 whitespace-nowrap text-slate-600">{formatDisplayDate(tx.bookingDate)}</td>
	<td class="py-2 pr-4">
		<button
			bind:this={opener}
			type="button"
			aria-haspopup="dialog"
			class="focus-visible:outline-brand rounded-sm text-left font-medium text-slate-900 hover:underline focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2"
		>
			{tx.counterpartyName ?? 'Unknown'}
		</button>
		{#if tx.note}
			<!-- A glyph, not a pill: the row already carries label-source and split pills. -->
			<span
				role="img"
				aria-label="Has a note: {tx.note}"
				title={tx.note}
				class="ml-1 cursor-help text-slate-400">✎</span
			>
		{/if}
	</td>
	<td class="py-2 pr-4 text-slate-600">{tx.description ?? ''}</td>
	<td class="py-2 pr-4">
		<div class="flex flex-wrap items-center gap-1">
			{#if split}
				<button
					type="button"
					data-split-toggle
					aria-expanded={partsOpen}
					aria-label={split.ariaLabel}
					onclick={() => (partsOpen = !partsOpen)}
					class="focus-visible:outline-brand inline-flex items-center gap-1 rounded-full bg-violet-100 px-2 py-0.5 text-xs font-medium text-violet-800 hover:bg-violet-200 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2"
				>
					<span aria-hidden="true">{partsOpen ? '▾' : '▸'}</span>
					{split.text}
				</button>
			{:else}
				<span class="text-slate-700">{tx.label?.category?.name ?? '—'}</span>
			{/if}
			{#if sourceBadge && !split}
				<Badge text={sourceBadge.text} variant={sourceBadge.variant} />
			{/if}
			{#if reviewPill}
				<a href="/review" class="inline-block">
					<Badge text={reviewPill.text} variant={reviewPill.variant} />
				</a>
			{/if}
		</div>
	</td>
	<td class="py-2 pr-4">
		<div class="flex flex-wrap gap-1">
			{#each tx.tags as tag (tag)}
				<Badge text={tag} variant="neutral" />
			{/each}
		</div>
	</td>
	<td class="py-2 pr-4">
		<div class="flex flex-wrap items-center gap-1">
			{#if tx.transfer}
				<Badge text="⇄ transfer" variant="info" />
			{/if}
			{#if tx.recurring.isRecurring}
				<Badge
					text={tx.recurring.source === 'USER' ? '↻ recurring · you' : '↻ recurring · auto'}
					variant={tx.recurring.source === 'USER' ? 'success' : 'info'}
				/>
			{/if}
		</div>
	</td>
	<td
		class="py-2 pr-0 text-right font-medium whitespace-nowrap"
		class:text-[color:var(--color-income)]={Number(tx.amount) > 0}
		class:text-[color:var(--color-spending)]={Number(tx.amount) < 0}
	>
		{formatAmount(tx.amount, currency)}
	</td>
</tr>

{#if split && partsOpen}
	<tr class="border-b border-slate-100 bg-violet-50/50" data-testid="split-parts">
		<td colspan={onselect ? 8 : 7} class="py-2 pr-0 pl-8">
			<p class="mb-1 text-xs text-slate-500">
				Split into {split.count}
				{split.count === 1 ? 'part' : 'parts'} — totals count each part under its own category.
			</p>
			<ul class="flex flex-col gap-0.5">
				{#each tx.splits as part (part.index)}
					<li class="flex max-w-md items-center justify-between gap-4 text-sm">
						<span class="text-slate-700">{part.category.name}</span>
						<span class="font-medium whitespace-nowrap text-slate-700">
							{formatAmount(part.amount, currency)}
						</span>
					</li>
				{/each}
			</ul>
		</td>
	</tr>
{/if}
