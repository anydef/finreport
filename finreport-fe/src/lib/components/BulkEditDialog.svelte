<script lang="ts">
	/**
	 * Bulk edit of one field across the table's selection. Both edits
	 * OVERRIDE what the transactions have now, and the dialog says so. A
	 * category change also discards splits, so it needs an explicit confirm
	 * when any selected transaction has them. The result of the call is shown
	 * as-is: a partial failure is a warning, never a success message.
	 */
	import Modal from './Modal.svelte';
	import SearchMenu from './SearchMenu.svelte';
	import TagEditor from './TagEditor.svelte';
	import { categoryOptionGroups } from '$lib/categoryTree';
	import { describeResult } from '$lib/bulkSelection';
	import type { BulkEditResult, Category } from '$lib/graphql/types';

	interface Props {
		kind: 'category' | 'tags';
		/** Exact number of transactions the edit will touch. */
		count: number;
		/** Selected transactions known to have splits; `exact: false` = lower bound. */
		splits: { count: number; exact: boolean };
		/** `null` while still loading. */
		categories: Category[] | null;
		onApplyCategory: (slug: string) => Promise<BulkEditResult>;
		onApplyTags: (tags: string[]) => Promise<BulkEditResult>;
		/** Extra explanation shown under the override note (e.g. what else the edit does). */
		note?: string;
		/** Called when the dialog closes after a result came back. */
		ondone: () => void;
		onclose: () => void;
	}

	let {
		kind,
		count,
		splits,
		categories,
		onApplyCategory,
		onApplyTags,
		note,
		ondone,
		onclose
	}: Props = $props();

	let slug = $state<string | null>(null);
	let tags = $state<string[]>([]);
	let busy = $state(false);
	let errorMessage = $state('');
	let result = $state<BulkEditResult | null>(null);

	const groups = $derived(categoryOptionGroups(categories ?? []));
	const slugLabel = $derived(
		groups.flatMap((g) => g.options).find((o) => o.slug === slug)?.label ?? slug
	);
	const outcome = $derived(result ? describeResult(result) : null);
	const noun = $derived(`${count} transaction${count === 1 ? '' : 's'}`);
	const hasSplits = $derived(kind === 'category' && (splits.count > 0 || !splits.exact));
	const canApply = $derived(kind === 'tags' || slug !== null);

	async function apply() {
		busy = true;
		errorMessage = '';
		try {
			result = kind === 'category' ? await onApplyCategory(slug!) : await onApplyTags(tags);
		} catch (err) {
			errorMessage = err instanceof Error ? err.message : 'The bulk edit could not be applied.';
		} finally {
			busy = false;
		}
	}

	const close = () => (result ? ondone() : onclose());

	const btn =
		'focus-visible:outline-brand rounded-md px-3 py-1.5 text-xs font-medium focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 disabled:opacity-50';
	const toneClass = {
		success: 'border-emerald-300 bg-emerald-50 text-emerald-900',
		partial: 'border-amber-400 bg-amber-50 text-amber-900',
		failed: 'border-red-300 bg-red-50 text-red-900'
	};
</script>

<Modal title={kind === 'category' ? 'Edit categories' : 'Edit tags'} onclose={close}>
	{#if result && outcome}
		<div
			role={outcome.tone === 'success' ? 'status' : 'alert'}
			data-testid="bulk-result"
			class="rounded-md border p-3 text-sm {toneClass[outcome.tone]}"
		>
			<p class="font-medium">{outcome.headline}</p>
			{#each outcome.details as line (line)}
				<p class="mt-1">{line}</p>
			{/each}
		</div>
		<div class="flex justify-end">
			<button
				type="button"
				onclick={ondone}
				class="{btn} bg-slate-100 text-slate-700 hover:bg-slate-200"
			>
				Close
			</button>
		</div>
	{:else}
		<p class="text-sm text-slate-700" data-testid="bulk-override-note">
			{#if kind === 'category'}
				This <strong>overrides</strong> the category of <strong>{noun}</strong>, replacing whatever
				each has now.
			{:else}
				This <strong>overrides</strong> the tags of <strong>{noun}</strong>: every existing tag is
				replaced by the set below. Leave it empty to remove all tags.
			{/if}
			There is no undo, but you can run another bulk edit to correct it.
		</p>
		{#if note}
			<p class="text-sm text-slate-700" data-testid="bulk-note">{note}</p>
		{/if}

		{#if kind === 'category'}
			{#if categories === null}
				<p class="text-sm text-slate-400">Loading categories…</p>
			{:else}
				<SearchMenu
					{groups}
					value={slug}
					onchange={(s) => (slug = s)}
					label="New category"
					placeholder="Choose a category…"
				/>
			{/if}
			{#if hasSplits}
				<div
					role="alert"
					class="rounded-md border border-amber-300 bg-amber-50 p-3 text-sm text-amber-900"
				>
					{#if splits.count > 0}
						{splits.count} of the selected transactions {splits.exact
							? 'have'
							: 'are known to have'}
						splits. Changing the category discards them.
					{:else}
						Some selected transactions may have splits; changing the category discards them.
					{/if}
					{#if !splits.exact}
						The exact number is only known after applying.
					{/if}
				</div>
			{/if}
		{:else}
			<TagEditor
				{tags}
				onSave={(next) => {
					tags = next;
				}}
			/>
		{/if}

		{#if errorMessage}
			<p role="alert" class="text-sm text-[var(--color-spending)]">{errorMessage}</p>
		{/if}

		<div class="flex justify-end gap-2">
			<button
				type="button"
				onclick={onclose}
				class="{btn} bg-slate-100 text-slate-700 hover:bg-slate-200"
			>
				Cancel
			</button>
			<button
				type="button"
				disabled={busy || !canApply}
				onclick={apply}
				class="{btn} {hasSplits
					? 'bg-amber-600 hover:bg-amber-700'
					: 'bg-brand hover:bg-brand/90'} text-white"
			>
				{#if kind === 'category'}
					{hasSplits ? 'Change category and discard splits' : 'Change category'}
					{slugLabel ? `to ${slugLabel}` : ''} for {noun}
				{:else}
					Replace tags on {noun}
				{/if}
			</button>
		</div>
	{/if}
</Modal>
