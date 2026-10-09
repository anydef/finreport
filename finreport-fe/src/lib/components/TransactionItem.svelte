<script lang="ts">
	/**
	 * One transaction row, shared by every transaction table. The whole row is
	 * the click target (mouse); the counterparty cell holds a real button so
	 * keyboard and screen-reader users get the same action and focus has
	 * somewhere to return to when the editor collapses. Clicking toggles the
	 * editor, rendered through the `editor` snippet in a full-width sub-row
	 * directly below. That sub-row is the row's single expansion area: the
	 * split pill is just another way to open it, and the split parts are
	 * listed inside the editor rather than in a second sub-row. Links inside
	 * the row (the "needs review" pill) keep their own behaviour.
	 */
	import type { Snippet } from 'svelte';
	import { formatAmount, formatDisplayDate } from '$lib/format';
	import { labelSourceBadge, needsReviewBadge } from '$lib/labelBadge';
	import { splitIndicator } from '$lib/splitView';
	import Badge from './Badge.svelte';
	import type { Transaction } from '$lib/graphql/types';
	import { counterpartyLabel, rawCounterpartyName } from '$lib/displayNames';

	interface Props {
		transaction: Transaction;
		currency: string;
		expanded?: boolean;
		/** Row clicked or activated from the keyboard: the owner opens or collapses the editor. */
		ontoggle: (transaction: Transaction) => void;
		/** Renders the editor; call `close` to collapse it and return focus to this row. */
		editor?: Snippet<[close: () => void]>;
		onclose?: () => void;
		/** Bulk-edit mode: render a checkbox cell. Omit `onselect` and there is none. */
		selected?: boolean;
		/** Ticked as part of "all matching": shown checked, not individually untickable. */
		selectLocked?: boolean;
		onselect?: () => void;
	}

	let {
		transaction: tx,
		currency,
		expanded = false,
		ontoggle,
		editor,
		onclose,
		selected = false,
		selectLocked = false,
		onselect
	}: Props = $props();

	const uid = $props.id();
	let opener = $state<HTMLButtonElement | null>(null);

	function onRowClick(event: MouseEvent) {
		if ((event.target as HTMLElement).closest('a, input, [data-select-cell]')) return;
		// Focus first so a mouse click leaves focus on this row's button too.
		opener?.focus();
		ontoggle(tx);
	}

	/** Collapse from inside the editor (Esc, Close): focus goes back to the row. */
	function collapse() {
		opener?.focus();
		onclose?.();
	}

	const sourceBadge = $derived(labelSourceBadge(tx.label));
	const reviewPill = $derived(needsReviewBadge(tx.label));
	// A split's own label has no category (it lives on the parts), so the cell
	// shows the split pill instead of "—". Its label source is always "you", so
	// that badge is dropped for splits: one pill, not two, keeps the row calm.
	const split = $derived(splitIndicator(tx.splits));
</script>

<!-- The counterparty button below is the keyboard/AT equivalent of this row click. -->
<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
<tr
	class="cursor-pointer border-b border-slate-100 last:border-0 focus-within:bg-slate-50 hover:bg-slate-50 {expanded
		? 'border-b-0 bg-slate-50'
		: ''}"
	onclick={onRowClick}
>
	{#if onselect}
		<td class="py-2 pr-2 pl-1" data-select-cell>
			<input
				type="checkbox"
				checked={selected}
				disabled={selectLocked}
				onchange={onselect}
				aria-label="Select transaction {counterpartyLabel(tx) ?? 'Unknown'} on {formatDisplayDate(
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
			aria-expanded={expanded}
			title={rawCounterpartyName(tx) ? `Bank name: ${rawCounterpartyName(tx)}` : undefined}
			aria-controls={expanded ? `${uid}-editor` : undefined}
			class="focus-visible:outline-brand rounded-sm text-left font-medium text-slate-900 hover:underline focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2"
		>
			{counterpartyLabel(tx) ?? 'Unknown'}
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
					aria-expanded={expanded}
					aria-label={split.ariaLabel}
					class="focus-visible:outline-brand inline-flex items-center gap-1 rounded-full bg-violet-100 px-2 py-0.5 text-xs font-medium text-violet-800 hover:bg-violet-200 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2"
				>
					<span aria-hidden="true">{expanded ? '▾' : '▸'}</span>
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

{#if expanded && editor}
	<tr id="{uid}-editor" class="border-b border-slate-100 bg-slate-50" data-testid="row-editor">
		<td colspan={onselect ? 8 : 7} class="p-0">
			{@render editor(collapse)}
		</td>
	</tr>
{/if}
