<script lang="ts">
	import Card from '$lib/components/Card.svelte';
	import Badge from '$lib/components/Badge.svelte';
	import { formatAmount, formatDisplayDate } from '$lib/format';
	import { isStale, sortByMonthlyEquivalentDesc } from '$lib/recurringView';
	import type { PageData } from './$types';

	let { data }: { data: PageData } = $props();

	const series = $derived(data.overview ? sortByMonthlyEquivalentDesc(data.overview.series) : []);
</script>

<svelte:head>
	<title>Recurring costs · finreport</title>
</svelte:head>

<div class="flex flex-col gap-6">
	{#if data.error}
		<p role="alert" class="text-sm text-[var(--color-spending)]">
			Failed to load recurring series from the GraphQL API.
		</p>
	{:else if data.overview}
		<Card>
			<p class="text-sm text-slate-500">Total monthly-equivalent cost</p>
			<p class="mt-1 text-2xl font-semibold text-[var(--color-spending)]">
				{formatAmount(`-${data.overview.totalMonthlyEquivalent}`, data.overview.currency)}
			</p>
		</Card>

		<Card>
			{#if series.length === 0}
				<p class="py-8 text-center text-sm text-slate-500">No recurring series detected yet.</p>
			{:else}
				<div class="overflow-x-auto">
					<table class="w-full text-sm">
						<thead>
							<tr class="border-b border-slate-200 text-left text-slate-500">
								<th class="py-2 pr-4 font-medium">Counterparty</th>
								<th class="py-2 pr-4 font-medium">Cadence</th>
								<th class="py-2 pr-4 text-right font-medium">Median amount</th>
								<th class="py-2 pr-4 text-right font-medium">Monthly equivalent</th>
								<th class="py-2 pr-4 text-right font-medium">Occurrences</th>
								<th class="py-2 pr-4 font-medium">Last</th>
								<th class="py-2 pr-4 font-medium">Next expected</th>
								<th class="py-2 pr-0 font-medium">Status</th>
							</tr>
						</thead>
						<tbody>
							{#each series as row (row.id)}
								<tr class="border-b border-slate-100 last:border-0 hover:bg-slate-50">
									<td class="py-2 pr-4">
										<a
											href={`/transactions?search=${encodeURIComponent(row.counterpartyName ?? row.counterpartyKey)}`}
											class="text-[var(--color-brand)] hover:underline"
										>
											{row.counterpartyName ?? row.counterpartyKey}
										</a>
									</td>
									<td class="py-2 pr-4 whitespace-nowrap text-slate-600">{row.cadence}</td>
									<td class="py-2 pr-4 text-right whitespace-nowrap"
										>{formatAmount(row.medianAmount, data.overview.currency)}</td
									>
									<td class="py-2 pr-4 text-right font-medium whitespace-nowrap"
										>{formatAmount(row.monthlyEquivalent, data.overview.currency)}</td
									>
									<td class="py-2 pr-4 text-right">{row.occurrenceCount}</td>
									<td class="py-2 pr-4 whitespace-nowrap text-slate-600"
										>{formatDisplayDate(row.lastDate)}</td
									>
									<td class="py-2 pr-4 whitespace-nowrap text-slate-600"
										>{formatDisplayDate(row.nextExpectedDate)}</td
									>
									<td class="py-2 pr-0">
										{#if row.stale || isStale(row.nextExpectedDate, row.cadence)}
											<Badge text="stale" variant="warning" />
										{:else}
											<Badge text="active" variant="success" />
										{/if}
									</td>
								</tr>
							{/each}
						</tbody>
					</table>
				</div>
			{/if}
		</Card>
	{/if}
</div>
