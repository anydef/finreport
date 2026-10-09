<script lang="ts">
	/**
	 * Dashboard category-breakdown card (§6): horizontal bars by top-level
	 * category for the selected period. Two separate actions per row:
	 *
	 * - the chevron button on the left expands a category that has
	 *   subcategories, fetching them on first open (`loadChildren`) and nesting
	 *   them under the parent; a level-2 row with children expands the same way;
	 * - the bar itself drills down: the owning page narrows the transaction
	 *   list (`onSelect`), for categories and for the synthetic Uncategorized /
	 *   Needs review rows alike. A child level's "(no subcategory)" row drills
	 *   into the parent's *own* transactions (`categorySlugsExact`), not its
	 *   children's.
	 *
	 * Plain Tailwind div bars rather than a LayerChart chart — this is a ranked
	 * list with a width-encoded magnitude, not an axis-based chart, and it needs
	 * no `{#if browser}` guard.
	 */
	import { formatAmount } from '$lib/format';
	import {
		hasChildCategories,
		maxBarAmount,
		shapeCategoryBreakdown,
		shapeChildBreakdown,
		type BreakdownBar
	} from '$lib/breakdownShaping';
	import type { Category, CategoryBreakdown } from '$lib/graphql/types';

	interface Props {
		breakdown: CategoryBreakdown;
		/** The taxonomy, to know which rows have children and how deep they are. */
		categories?: Category[];
		/** Fetch one category's child breakdown; without it no row is expandable. */
		loadChildren?: (slug: string) => Promise<CategoryBreakdown>;
		/** Changes whenever the breakdown's scope does; expanded rows are re-fetched then. */
		scopeKey?: string;
		onSelect?: (bar: BreakdownBar) => void;
	}

	let { breakdown, categories = [], loadChildren, scopeKey = '', onSelect }: Props = $props();

	type ChildState =
		| { status: 'loading' }
		| { status: 'error' }
		| { status: 'ready'; bars: BreakdownBar[] };

	let expanded = $state<Record<string, boolean>>({});
	let children = $state<Record<string, ChildState>>({});
	let generation = 0;
	let lastScope = scopeKey;

	// A different period/filter makes every fetched child list stale. Expanded
	// rows stay open (a drill-down click reloads the page data but not the
	// scope) and refetch lazily.
	$effect(() => {
		if (scopeKey === lastScope) return;
		lastScope = scopeKey;
		generation++;
		children = {};
		for (const slug of Object.keys(expanded)) if (expanded[slug]) void fetchChildren(slug);
	});

	const bars = $derived(shapeCategoryBreakdown(breakdown));

	function barColor(bar: BreakdownBar): string {
		if (bar.kind === 'uncategorized') return 'bg-slate-300';
		if (bar.kind === 'needs-review') return 'bg-amber-300';
		return 'bg-[var(--color-brand)]';
	}

	function canExpand(bar: BreakdownBar): boolean {
		return (
			Boolean(loadChildren) &&
			bar.kind === 'category' &&
			!bar.own &&
			bar.slug !== null &&
			hasChildCategories(bar.slug, categories)
		);
	}

	async function fetchChildren(slug: string) {
		if (!loadChildren) return;
		const mine = generation;
		children[slug] = { status: 'loading' };
		try {
			const result = await loadChildren(slug);
			if (mine !== generation) return;
			children[slug] = { status: 'ready', bars: shapeChildBreakdown(result, slug) };
		} catch {
			if (mine !== generation) return;
			children[slug] = { status: 'error' };
		}
	}

	function toggle(slug: string) {
		expanded[slug] = !expanded[slug];
		// Fetch on first open, and retry after a failure.
		if (expanded[slug] && (!children[slug] || children[slug].status === 'error')) {
			void fetchChildren(slug);
		}
	}

	const focusRing =
		'focus-visible:outline-brand focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2';
</script>

{#snippet rowBody(bar: BreakdownBar, maxAmount: number)}
	<span class="flex items-center justify-between gap-2 text-sm">
		<span class="font-medium text-slate-700">
			{bar.label}
			{#if bar.kind !== 'category'}
				<span class="text-xs font-normal text-slate-400">
					({bar.kind === 'uncategorized' ? 'no category' : 'held'})
				</span>
			{/if}
		</span>
		<span class="shrink-0 text-slate-600">
			{formatAmount(String(bar.amount), breakdown.currency)} · {(bar.share * 100).toFixed(0)}%
		</span>
	</span>
	<span class="h-2 w-full overflow-hidden rounded-full bg-slate-100">
		<span
			class="block h-full rounded-full {barColor(bar)}"
			style="width: {maxAmount > 0 ? (bar.amount / maxAmount) * 100 : 0}%"
		></span>
	</span>
{/snippet}

{#snippet tree(list: BreakdownBar[])}
	{@const maxAmount = maxBarAmount(list)}
	<ul class="flex flex-col gap-2">
		{#each list as bar (bar.key)}
			{@const expandable = canExpand(bar)}
			{@const open = expandable && bar.slug !== null && expanded[bar.slug] === true}
			<li>
				<div class="flex items-start gap-1">
					{#if expandable && bar.slug !== null}
						{@const slug = bar.slug}
						<button
							type="button"
							class="mt-0.5 flex h-7 w-7 shrink-0 items-center justify-center rounded border border-slate-300 bg-white text-slate-600 hover:bg-slate-100 {focusRing}"
							aria-expanded={open}
							aria-controls="breakdown-children-{slug}"
							aria-label="{open ? 'Collapse' : 'Expand'} {bar.label} subcategories"
							title="{open ? 'Collapse' : 'Expand'} subcategories"
							onclick={() => toggle(slug)}
						>
							<svg
								class="h-4 w-4 transition-transform {open ? 'rotate-90' : ''}"
								viewBox="0 0 20 20"
								fill="currentColor"
								aria-hidden="true"
							>
								<path d="M7 4l6 6-6 6z" />
							</svg>
						</button>
					{:else}
						<span class="h-7 w-7 shrink-0" aria-hidden="true"></span>
					{/if}
					<button
						type="button"
						class="flex w-full min-w-0 flex-col gap-1 rounded px-1 py-1 text-left hover:bg-slate-50 {focusRing}"
						title="Show transactions: {bar.label}"
						onclick={() => onSelect?.(bar)}
					>
						{@render rowBody(bar, maxAmount)}
					</button>
				</div>
				{#if open && bar.slug !== null}
					{@const state = children[bar.slug]}
					<div
						id="breakdown-children-{bar.slug}"
						class="mt-1 ml-3 border-l-2 border-slate-200 pl-3"
					>
						{#if !state || state.status === 'loading'}
							<p role="status" class="py-1 text-sm text-slate-400">Loading subcategories…</p>
						{:else if state.status === 'error'}
							<p role="alert" class="py-1 text-sm text-[var(--color-spending)]">
								Could not load subcategories. Collapse and expand to retry.
							</p>
						{:else if state.bars.length === 0}
							<p class="py-1 text-sm text-slate-500">No subcategory spending in this period.</p>
						{:else}
							{@render tree(state.bars)}
						{/if}
					</div>
				{/if}
			</li>
		{/each}
	</ul>
{/snippet}

{#if bars.length === 0}
	<p class="py-8 text-center text-sm text-slate-500">No categorized spending in this period.</p>
{:else}
	{#if loadChildren}
		<p class="mb-2 text-xs text-slate-400">
			Click a row to list its transactions. Use the arrow to show its subcategories.
		</p>
	{/if}
	{@render tree(bars)}
{/if}
