<script lang="ts">
	import type { Category, Transaction } from '$lib/graphql/types';
	import SearchMenu from '$lib/components/SearchMenu.svelte';
	import { formatAmount, formatDisplayDate } from '$lib/format';

	interface Props {
		transaction: Transaction;
		categories: Category[];
		onSetCategory: (categorySlug: string) => Promise<void>;
		onCreateAndApplyProposed: (proposedPath: string) => Promise<void>;
		onOpenSplitEditor: () => void;
	}

	let {
		transaction,
		categories,
		onSetCategory,
		onCreateAndApplyProposed,
		onOpenSplitEditor
	}: Props = $props();

	/**
	 * Small local category `<select>`, grouped by top-level ancestor path —
	 * deliberately not the shared `CategoryPicker` (owned/still in progress
	 * in the analytics package, WP5) so this card has no dependency on it.
	 */
	function groupedOptions(cats: Category[]) {
		const byId = new Map(cats.map((c) => [c.id, c]));
		function pathOf(cat: Category): string {
			return cat.parentId && byId.has(cat.parentId)
				? `${pathOf(byId.get(cat.parentId)!)} / ${cat.name}`
				: cat.name;
		}
		const groups = new Map<string, { slug: string; label: string }[]>();
		for (const cat of cats) {
			if (cat.archived) continue;
			const top =
				cat.parentId && byId.has(cat.parentId)
					? pathOf(byId.get(cat.parentId)!).split(' / ')[0]
					: cat.name;
			const list = groups.get(top) ?? [];
			list.push({ slug: cat.slug, label: pathOf(cat) });
			groups.set(top, list);
		}
		return [...groups.entries()].map(([groupLabel, options]) => ({
			groupLabel,
			options: options.sort((a, b) => a.label.localeCompare(b.label))
		}));
	}

	const optionGroups = $derived(groupedOptions(categories));

	let picked = $state('');
	let suggestionDismissed = $state(false);
	let busy = $state(false);
	let errorMessage = $state('');

	const label = $derived(transaction.label);
	const reason = $derived(
		label?.reviewReason ?? (label?.status === 'NEEDS_REVIEW' ? 'OTHER' : null)
	);
	const reasonText = $derived(
		reason === 'AMBIGUOUS'
			? 'Ambiguous'
			: reason === 'NEW_CATEGORY'
				? 'New category suggested'
				: reason === 'OTHER'
					? 'Needs review (split mismatch)'
					: ''
	);
	const confidencePct = $derived(
		label?.confidence != null ? `${Math.round(label.confidence * 100)}%` : null
	);

	async function runAction(action: () => Promise<void>) {
		busy = true;
		errorMessage = '';
		try {
			await action();
		} catch (err) {
			errorMessage = err instanceof Error ? err.message : 'Action failed';
		} finally {
			busy = false;
		}
	}

	function pickCategory() {
		if (!picked) return;
		void runAction(() => onSetCategory(picked));
	}

	function acceptSuggestion() {
		if (!label?.proposedCategoryPath) return;
		void runAction(() => onCreateAndApplyProposed(label.proposedCategoryPath!));
	}
</script>

<div class="flex flex-col gap-3 rounded-lg border border-slate-200 p-4">
	<div class="flex flex-wrap items-start justify-between gap-2">
		<div>
			<p class="text-sm font-medium text-slate-900">
				{transaction.counterpartyName ?? transaction.description ?? 'Unknown counterparty'}
			</p>
			<p class="text-xs text-slate-500">{formatDisplayDate(transaction.bookingDate)}</p>
		</div>
		<p class="text-sm font-semibold text-slate-900">
			{formatAmount(transaction.amount, transaction.currency)}
		</p>
	</div>

	{#if reasonText}
		<p class="text-xs font-medium text-[var(--color-spending)]">
			<span aria-hidden="true">⚠</span>
			{reasonText}{confidencePct ? ` · confidence ${confidencePct}` : ''}
		</p>
	{/if}
	{#if label?.reasoning}
		<p class="text-xs text-slate-600">{label.reasoning}</p>
	{/if}

	{#if label?.reviewReason === 'NEW_CATEGORY' && label.proposedCategoryPath && !suggestionDismissed}
		<div class="flex flex-wrap items-center gap-2 rounded-md bg-slate-50 p-2 text-xs">
			<span>Suggested new category: <strong>{label.proposedCategoryPath}</strong></span>
			<button
				type="button"
				disabled={busy}
				onclick={acceptSuggestion}
				class="focus-visible:outline-brand bg-brand hover:bg-brand/90 rounded-md px-2 py-1 font-medium text-white focus-visible:outline focus-visible:outline-2 disabled:opacity-50"
			>
				Approve &amp; create
			</button>
			<button
				type="button"
				disabled={busy}
				onclick={() => (suggestionDismissed = true)}
				class="focus-visible:outline-brand rounded-md bg-slate-100 px-2 py-1 font-medium text-slate-700 hover:bg-slate-200 focus-visible:outline focus-visible:outline-2 disabled:opacity-50"
			>
				Reject
			</button>
		</div>
	{/if}

	<div class="flex flex-wrap items-center gap-2">
		<span class="text-xs text-slate-600">Pick a category</span>
		<SearchMenu
			groups={optionGroups}
			value={picked || null}
			onchange={(slug) => (picked = slug)}
			label="Pick a category"
			placeholder="Select…"
		/>
		<button
			type="button"
			disabled={busy || !picked}
			onclick={pickCategory}
			class="focus-visible:outline-brand rounded-md bg-slate-100 px-2 py-1 text-xs font-medium text-slate-700 hover:bg-slate-200 focus-visible:outline focus-visible:outline-2 disabled:opacity-50"
		>
			Set category
		</button>
		<button
			type="button"
			disabled={busy}
			onclick={onOpenSplitEditor}
			class="focus-visible:outline-brand rounded-md bg-slate-100 px-2 py-1 text-xs font-medium text-slate-700 hover:bg-slate-200 focus-visible:outline focus-visible:outline-2 disabled:opacity-50"
		>
			{(transaction.splits ?? []).length > 0 ? 'Edit split' : 'Split'}
		</button>
	</div>

	{#if errorMessage}
		<p role="alert" class="text-xs text-[var(--color-spending)]">{errorMessage}</p>
	{/if}
</div>
