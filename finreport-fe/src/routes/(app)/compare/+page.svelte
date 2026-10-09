<script lang="ts">
	import { browser } from '$app/environment';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import Card from '$lib/components/Card.svelte';
	import ComparisonTrendChart from '$lib/components/ComparisonTrendChart.svelte';
	import Field from '$lib/components/Field.svelte';
	import {
		SPAN_OPTIONS,
		defaultPair,
		formatDeltaPercent,
		selectableEndMonths,
		transactionsHref,
		trendBars,
		type Delta,
		type TrendBar
	} from '$lib/comparisonView';
	import { formatAmount } from '$lib/format';
	import type { PageData } from './$types';

	let { data }: { data: PageData } = $props();

	const view = $derived(data.view);
	const endMonths = $derived(selectableEndMonths(new Date(data.today + 'T00:00:00')));
	const bars = $derived(view ? trendBars(view) : []);
	const isDefaultPair = $derived(
		view
			? (() => {
					const d = defaultPair(view.periods.length, data.window.partial);
					return d.base === view.baseIndex && d.current === view.currentIndex;
				})()
			: true
	);

	interface Nav {
		months?: number;
		/** "YYYY-MM", or '' for the current month. */
		end?: string;
		/** Period start dates; omitted keeps the current choice. */
		base?: string;
		to?: string;
		/** Changing the window invalidates the picked pair. */
		resetPair?: boolean;
	}

	function go(next: Nav) {
		const months = next.months ?? data.span;
		const end = next.end ?? (data.window.partial ? '' : data.window.endMonth);
		const params = new URLSearchParams();
		if (months !== 6) params.set('months', String(months));
		if (end) params.set('end', end);
		if (!next.resetPair && view) {
			const base = next.base ?? view.periods[view.baseIndex]?.start;
			const to = next.to ?? view.periods[view.currentIndex]?.start;
			const d = defaultPair(view.periods.length, data.window.partial);
			const baseIsDefault = base === view.periods[d.base]?.start;
			const toIsDefault = to === view.periods[d.current]?.start;
			if (!(baseIsDefault && toIsDefault)) {
				if (base) params.set('base', base);
				if (to) params.set('to', to);
			}
		}
		goto(`${page.url.pathname}?${params.toString()}`, { keepFocus: true, noScroll: true });
	}

	function onBarClick(bar: TrendBar) {
		go({ to: bar.start });
	}

	function money(value: number): string {
		return formatAmount(String(Math.abs(value)), view?.currency ?? 'EUR').replace(/^[+−]/, '');
	}

	function signed(value: number): string {
		return formatAmount(String(value), view?.currency ?? 'EUR');
	}

	/** Increases in spending read as bad, decreases as good; the sign is always printed too. */
	function tone(d: Delta): string {
		if (d.kind === 'up' || d.kind === 'new') return 'text-[var(--color-spending)]';
		if (d.kind === 'down' || d.kind === 'gone') return 'text-[var(--color-income)]';
		return 'text-slate-500';
	}

	const selectClass = 'rounded-md border-slate-300 text-sm';
	const linkClass =
		'focus-visible:outline-brand underline decoration-slate-300 underline-offset-2 hover:decoration-slate-500 focus-visible:outline focus-visible:outline-2';
</script>

<svelte:head>
	<title>Compare · finreport</title>
</svelte:head>

<div class="flex flex-col gap-6">
	<Card>
		<form
			class="flex flex-wrap items-end gap-4"
			onsubmit={(e) => {
				e.preventDefault();
			}}
		>
			<Field label="Months" for="cmp-span">
				<select
					id="cmp-span"
					class={selectClass}
					value={data.span}
					onchange={(e) => go({ months: Number(e.currentTarget.value), resetPair: true })}
				>
					{#each SPAN_OPTIONS as n (n)}
						<option value={n}>Last {n}</option>
					{/each}
				</select>
			</Field>
			<Field label="Ending" for="cmp-end">
				<select
					id="cmp-end"
					class={selectClass}
					value={data.window.partial ? '' : data.window.endMonth}
					onchange={(e) => go({ end: e.currentTarget.value, resetPair: true })}
				>
					<option value="">This month (so far)</option>
					{#each endMonths as m (m.id)}
						<option value={m.id.slice('month:'.length)}>{m.label}</option>
					{/each}
				</select>
			</Field>
			{#if view}
				<Field label="Compare" for="cmp-base">
					<select
						id="cmp-base"
						class={selectClass}
						value={view.periods[view.baseIndex]?.start}
						onchange={(e) => go({ base: e.currentTarget.value })}
					>
						{#each view.periods as p (p.start)}
							<option value={p.start}>{p.label}{p.partial ? ' (so far)' : ''}</option>
						{/each}
					</select>
				</Field>
				<Field label="To" for="cmp-to">
					<select
						id="cmp-to"
						class={selectClass}
						value={view.periods[view.currentIndex]?.start}
						onchange={(e) => go({ to: e.currentTarget.value })}
					>
						{#each view.periods as p (p.start)}
							<option value={p.start}>{p.label}{p.partial ? ' (so far)' : ''}</option>
						{/each}
					</select>
				</Field>
				{#if !isDefaultPair}
					<a
						href="{page.url.pathname}?{new URLSearchParams(
							[...page.url.searchParams].filter(([k]) => k !== 'base' && k !== 'to')
						).toString()}"
						class="text-sm text-slate-600 underline">Reset to latest two</a
					>
				{/if}
			{/if}
		</form>
	</Card>

	{#if data.error || !view}
		<p role="alert" class="text-sm text-[var(--color-spending)]">
			Failed to load the comparison from the GraphQL API.
		</p>
	{:else}
		{@const base = view.periods[view.baseIndex]}
		{@const current = view.periods[view.currentIndex]}
		<section aria-label="Total change" class="grid grid-cols-1 gap-4 sm:grid-cols-3">
			<Card title={base.label}>
				<p class="text-2xl font-semibold" data-testid="total-base">{money(view.total.base)}</p>
				<p class="text-xs text-slate-500">total spending</p>
			</Card>
			<Card title={current.label + (current.partial ? ' (so far)' : '')}>
				<p class="text-2xl font-semibold" data-testid="total-current">
					{money(view.total.current)}
				</p>
				<p class="text-xs text-slate-500">total spending</p>
			</Card>
			<Card title="Change">
				<p class="text-2xl font-semibold {tone(view.total.delta)}" data-testid="total-delta">
					{signed(view.total.delta.abs)}
					<span class="text-base font-medium">({formatDeltaPercent(view.total.delta)})</span>
				</p>
				{#if current.partial}
					<p class="text-xs text-slate-500">
						{current.label} is not over yet, so it will read low until it is.
					</p>
				{/if}
			</Card>
		</section>

		<Card title="Total spending per month">
			{#if browser}
				<ComparisonTrendChart {bars} currency={view.currency} {onBarClick} />
			{:else}
				<p class="flex h-64 items-center justify-center text-sm text-slate-400">Loading chart…</p>
			{/if}
			<p class="mt-2 text-xs text-slate-500">
				Click a bar to compare against that month.{current.partial
					? ' * The current month so far.'
					: ''}
			</p>
		</Card>

		<Card title="What changed, by category">
			{#if view.rows.length === 0 && view.uncategorized.current === 0 && view.uncategorized.base === 0}
				<p class="text-sm text-slate-500">No spending in this window.</p>
			{:else}
				<div class="overflow-x-auto">
					<table class="w-full text-sm" data-testid="comparison-table">
						<thead>
							<tr class="border-b border-slate-200 text-left text-xs text-slate-500 uppercase">
								<th scope="col" class="py-2 pr-4 font-medium">Category</th>
								<th scope="col" class="px-2 py-2 text-right font-medium">Change</th>
								<th scope="col" class="px-2 py-2 text-right font-medium">%</th>
								{#each view.periods as p, i (p.start)}
									<th
										scope="col"
										class="px-2 py-2 text-right font-medium whitespace-nowrap {i ===
											view.baseIndex || i === view.currentIndex
											? 'text-slate-900'
											: ''}"
									>
										{p.shortLabel}{p.partial ? '*' : ''}
									</th>
								{/each}
							</tr>
						</thead>
						<tbody>
							{#each view.rows as row (row.slug)}
								<tr class="border-b border-slate-100" data-testid="row-{row.slug}">
									<th scope="row" class="py-2 pr-4 text-left font-medium whitespace-nowrap">
										{row.name}
										{#if row.delta.kind === 'new'}
											<span
												class="ml-1 rounded bg-slate-100 px-1.5 py-0.5 text-xs font-medium text-slate-700"
												>new</span
											>
										{:else if row.delta.kind === 'gone'}
											<span
												class="ml-1 rounded bg-slate-100 px-1.5 py-0.5 text-xs font-medium text-slate-700"
												>gone</span
											>
										{/if}
									</th>
									<td class="px-2 py-2 text-right whitespace-nowrap {tone(row.delta)}">
										{row.delta.kind === 'none' ? '–' : signed(row.delta.abs)}
									</td>
									<td class="px-2 py-2 text-right whitespace-nowrap {tone(row.delta)}">
										{formatDeltaPercent(row.delta)}
									</td>
									{#each view.periods as p, i (p.start)}
										<td
											class="px-2 py-2 text-right whitespace-nowrap {i === view.baseIndex ||
											i === view.currentIndex
												? 'bg-slate-50 font-medium'
												: 'text-slate-600'}"
										>
											{#if row.counts[i] > 0}
												<a
													href={transactionsHref(p, data.today, { slug: row.slug })}
													class={linkClass}
													aria-label="{row.name}, {p.label}: {money(
														row.cells[i]
													)}, show transactions"
												>
													{money(row.cells[i])}
												</a>
											{:else}
												<span class="text-slate-400">&ndash;</span>
											{/if}
										</td>
									{/each}
								</tr>
							{/each}
							{#if view.uncategorized.base > 0 || view.uncategorized.current > 0}
								<tr class="border-b border-slate-100 text-slate-600">
									<th scope="row" class="py-2 pr-4 text-left font-medium whitespace-nowrap">
										Uncategorized
									</th>
									<td
										class="px-2 py-2 text-right whitespace-nowrap {tone(view.uncategorized.delta)}"
									>
										{view.uncategorized.delta.kind === 'none'
											? '–'
											: signed(view.uncategorized.delta.abs)}
									</td>
									<td
										class="px-2 py-2 text-right whitespace-nowrap {tone(view.uncategorized.delta)}"
									>
										{formatDeltaPercent(view.uncategorized.delta)}
									</td>
									{#each view.periods as p, i (p.start)}
										<td class="px-2 py-2 text-right whitespace-nowrap">
											<a
												href={transactionsHref(p, data.today, { uncategorized: true })}
												class={linkClass}
											>
												{money(p.uncategorized)}
											</a>
										</td>
									{/each}
								</tr>
							{/if}
						</tbody>
						<tfoot>
							<tr class="border-t border-slate-300 font-semibold">
								<th scope="row" class="py-2 pr-4 text-left">Total</th>
								<td class="px-2 py-2 text-right whitespace-nowrap {tone(view.total.delta)}">
									{view.total.delta.kind === 'none' ? '–' : signed(view.total.delta.abs)}
								</td>
								<td class="px-2 py-2 text-right whitespace-nowrap {tone(view.total.delta)}">
									{formatDeltaPercent(view.total.delta)}
								</td>
								{#each view.periods as p (p.start)}
									<td class="px-2 py-2 text-right whitespace-nowrap">{money(p.total)}</td>
								{/each}
							</tr>
						</tfoot>
					</table>
				</div>
				<p class="mt-3 text-xs text-slate-500">
					Amounts are net of refunds. Transfers are not counted, and transactions held for review
					are shown below rather than folded into a category.
				</p>
			{/if}
		</Card>

		{#if view.needsReview.base > 0 || view.needsReview.current > 0}
			<Card title="Held for review (not included above)">
				<p class="text-sm text-slate-600">
					{base.label}: {money(view.needsReview.base)} &middot; {current.label}:
					{money(view.needsReview.current)}.
					<a
						href={transactionsHref(current, data.today, { needsReview: true })}
						class="{linkClass} ml-1"
					>
						Show {current.label} held transactions
					</a>
				</p>
			</Card>
		{/if}
	{/if}
</div>
