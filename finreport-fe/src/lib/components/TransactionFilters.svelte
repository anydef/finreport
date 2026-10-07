<script lang="ts">
	/**
	 * The one transaction filter panel, shared by the dashboard and
	 * `/transactions` so the two cannot drift. Controlled: it renders `value`
	 * and reports each edit through `onchange(next)`; the page turns that into
	 * search params (`transactionFilters.ts`), so a filtered view stays linkable.
	 */
	import type { Snippet } from 'svelte';
	import Tree from './Tree.svelte';
	import Button from './Button.svelte';
	import Card from './Card.svelte';
	import { activeCategories, buildCategoryTree } from '$lib/categoryTree';
	import type { Account, Category, TagCount } from '$lib/graphql/types';
	import {
		FLAG_KEYS,
		activeFilters,
		clearedFilters,
		isAmountRangeInverted,
		normalizeAmount,
		removeFilter,
		triFromValue,
		triToValue,
		type FlagKey,
		type PanelFilters,
		type TriValue
	} from '$lib/transactionFilters';

	interface Props {
		value: PanelFilters;
		accounts: Account[];
		categories: Category[];
		tags: TagCount[];
		/** Rows matching, across all pages; `undefined` while unknown. */
		matchCount?: number;
		/** What the filters narrow on this page, stated next to the controls. */
		scopeNote?: string;
		onchange: (next: PanelFilters) => void;
		/** Page-specific controls rendered first (e.g. the period on `/transactions`). */
		lead?: Snippet;
	}

	let { value, accounts, categories, tags, matchCount, scopeNote, onchange, lead }: Props =
		$props();

	const accountLabel = (a: Account) => a.label ?? a.displayId ?? a.id;
	const tree = $derived(buildCategoryTree(activeCategories(categories)));
	const chips = $derived(
		activeFilters(value, {
			accounts: accounts.map((a) => ({ id: a.id, label: accountLabel(a) })),
			categories
		})
	);
	const inverted = $derived(isAmountRangeInverted(value));

	// Text inputs commit on change (Enter or blur), not per keystroke, so typing
	// does not reload the page; the drafts follow the URL when it changes.
	let searchDraft = $state('');
	let minDraft = $state('');
	let maxDraft = $state('');
	$effect(() => {
		searchDraft = value.search;
		minDraft = value.amountMin ?? '';
		maxDraft = value.amountMax ?? '';
	});

	function commitText() {
		onchange({
			...value,
			search: searchDraft.trim(),
			amountMin: normalizeAmount(minDraft),
			amountMax: normalizeAmount(maxDraft)
		});
	}

	function toggle(list: string[], item: string): string[] {
		return list.includes(item) ? list.filter((i) => i !== item) : [...list, item];
	}

	const FLAGS: { key: FlagKey; label: string; yes: string; no: string }[] = [
		{ key: 'recurring', label: 'Recurring', yes: 'Recurring', no: 'Not recurring' },
		{ key: 'transfer', label: 'Transfer', yes: 'Transfers', no: 'Not transfers' },
		{ key: 'needsReview', label: 'Needs review', yes: 'Needs review', no: 'Reviewed' },
		{ key: 'uncategorized', label: 'Uncategorized', yes: 'Uncategorized', no: 'Categorized' }
	];
	const advancedActive = $derived(
		value.categorySlugs.length > 0 ||
			value.tags.length > 0 ||
			FLAG_KEYS.some((k) => value[k] !== undefined)
	);

	const input =
		'rounded-md border border-slate-300 px-2 py-1.5 text-sm focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-brand';
	const segment =
		'cursor-pointer select-none px-2.5 py-1 text-xs font-medium text-slate-600 transition-colors hover:bg-slate-100 peer-checked:bg-brand peer-checked:text-white peer-focus-visible:outline peer-focus-visible:outline-2 peer-focus-visible:outline-offset-1 peer-focus-visible:outline-brand';
</script>

<Card>
	<section aria-label="Transaction filters" class="flex flex-col gap-4">
		<form
			class="flex flex-wrap items-end gap-4"
			onsubmit={(e) => {
				e.preventDefault();
				commitText();
			}}
		>
			{@render lead?.()}
			<label class="flex flex-col gap-1 text-sm font-medium text-slate-700" for="tf-search">
				Search
				<input
					id="tf-search"
					type="search"
					bind:value={searchDraft}
					onchange={commitText}
					placeholder="Counterparty or description"
					class="{input} w-64"
				/>
			</label>
			<fieldset class="flex flex-col gap-1">
				<legend class="text-sm font-medium text-slate-700">Amount (absolute)</legend>
				<div class="flex items-center gap-2">
					<input
						id="tf-amount-min"
						type="text"
						inputmode="decimal"
						aria-label="Minimum amount"
						bind:value={minDraft}
						onchange={commitText}
						placeholder="Min"
						aria-invalid={inverted}
						class="{input} w-24"
					/>
					<span class="text-slate-400" aria-hidden="true">to</span>
					<input
						id="tf-amount-max"
						type="text"
						inputmode="decimal"
						aria-label="Maximum amount"
						bind:value={maxDraft}
						onchange={commitText}
						placeholder="Max"
						aria-invalid={inverted}
						class="{input} w-24"
					/>
				</div>
			</fieldset>
			{#if accounts.length > 0}
				<fieldset class="flex flex-col gap-1">
					<legend class="text-sm font-medium text-slate-700">Accounts</legend>
					<div class="flex flex-wrap gap-x-3 gap-y-1">
						{#each accounts as account (account.id)}
							<label class="flex cursor-pointer items-center gap-1.5 text-sm">
								<input
									type="checkbox"
									class="focus-visible:outline-brand size-4 cursor-pointer rounded border-slate-300 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-1"
									checked={value.accountIds.includes(account.id)}
									onchange={() =>
										onchange({ ...value, accountIds: toggle(value.accountIds, account.id) })}
								/>
								{accountLabel(account)}
							</label>
						{/each}
					</div>
				</fieldset>
			{/if}
		</form>
		{#if inverted}
			<p role="alert" class="text-sm text-[var(--color-spending)]">
				The minimum amount is larger than the maximum, so nothing can match.
			</p>
		{/if}

		<details open={advancedActive} class="group rounded-md border border-slate-200">
			<summary
				class="focus-visible:outline-brand cursor-pointer rounded-md px-3 py-2 text-sm font-medium text-slate-700 hover:bg-slate-50 focus-visible:outline focus-visible:outline-2"
			>
				Category, tags and flags
				{#if advancedActive}<span class="text-brand ml-1 text-xs font-normal">(in use)</span>{/if}
			</summary>
			<div class="flex flex-wrap items-start gap-8 border-t border-slate-200 p-3">
				{#if tree.length > 0}
					<div class="flex flex-col gap-1">
						<p id="tf-category-label" class="text-sm font-medium text-slate-700">
							Categories <span class="font-normal text-slate-500">(includes subcategories)</span>
						</p>
						<div
							role="group"
							aria-labelledby="tf-category-label"
							class="max-h-48 overflow-y-auto rounded-md border border-slate-200 p-2"
						>
							<Tree
								nodes={tree}
								selected={value.categorySlugs}
								mode="multi"
								onToggle={(slug) =>
									onchange({ ...value, categorySlugs: toggle(value.categorySlugs, slug) })}
							/>
						</div>
					</div>
				{/if}
				<fieldset class="flex max-w-sm flex-col gap-1">
					<legend class="text-sm font-medium text-slate-700">
						Tags <span class="font-normal text-slate-500">(must have all selected)</span>
					</legend>
					{#if tags.length === 0}
						<p class="text-sm text-slate-400">No tags yet.</p>
					{:else}
						<div class="flex flex-wrap gap-x-3 gap-y-1">
							{#each tags as tagCount (tagCount.tag)}
								<label class="flex cursor-pointer items-center gap-1.5 text-sm">
									<input
										type="checkbox"
										class="focus-visible:outline-brand size-4 cursor-pointer rounded border-slate-300 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-1"
										checked={value.tags.includes(tagCount.tag)}
										onchange={() => onchange({ ...value, tags: toggle(value.tags, tagCount.tag) })}
									/>
									{tagCount.tag} ({tagCount.transactionCount})
								</label>
							{/each}
						</div>
					{/if}
				</fieldset>
				<div class="flex flex-col gap-2">
					{#each FLAGS as flag (flag.key)}
						{@const current = triToValue(value[flag.key])}
						<fieldset class="flex items-center gap-3">
							<legend class="sr-only">{flag.label}</legend>
							<span class="w-28 text-sm text-slate-700" aria-hidden="true">{flag.label}</span>
							<div
								class="inline-flex divide-x divide-slate-300 overflow-hidden rounded-md border border-slate-300"
							>
								{#each [['any', 'Any'], ['yes', flag.yes], ['no', flag.no]] as [v, text] (v)}
									<label class="relative">
										<input
											type="radio"
											class="peer sr-only"
											name="tf-flag-{flag.key}"
											value={v}
											checked={current === v}
											onchange={() =>
												onchange({ ...value, [flag.key]: triFromValue(v as TriValue) })}
										/>
										<span class="{segment} block">{text}</span>
									</label>
								{/each}
							</div>
						</fieldset>
					{/each}
				</div>
			</div>
		</details>

		<div class="flex flex-wrap items-center gap-2" aria-live="polite">
			<p class="text-sm font-medium text-slate-700">
				{#if matchCount === undefined}
					Filters
				{:else}
					{matchCount} matching transaction{matchCount === 1 ? '' : 's'}
				{/if}
			</p>
			{#each chips as chip (chip.id)}
				<button
					type="button"
					onclick={() => onchange(removeFilter(value, chip.id))}
					aria-label="Remove filter: {chip.label}"
					class="bg-brand/10 text-brand hover:bg-brand/20 focus-visible:outline-brand inline-flex cursor-pointer items-center gap-1 rounded-full px-2.5 py-0.5 text-xs font-medium transition-colors focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-1"
				>
					{chip.label}
					<span aria-hidden="true">&times;</span>
				</button>
			{/each}
			{#if chips.length > 0}
				<Button variant="ghost" onclick={() => onchange(clearedFilters())}>Clear all filters</Button
				>
			{:else}
				<span class="text-xs text-slate-500">No filters applied.</span>
			{/if}
		</div>
		{#if scopeNote}
			<p class="text-xs text-slate-500">{scopeNote}</p>
		{/if}
	</section>
</Card>
