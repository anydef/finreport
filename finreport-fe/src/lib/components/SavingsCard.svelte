<!--
  The savings card: net change across the accounts the user marked as savings.

  "Net balance change" semantics, chosen deliberately: transfers in,
  withdrawals out, interest or salary paid straight in, and a payment made
  directly from a savings account all count. A movement between two savings
  accounts counts as neither, because it changes no total.

  The consequence is stated on the card rather than left for the reader to
  discover: a payment made straight from savings is in this figure *and* in
  the spending card, so the two are each correct but are not a partition.
-->
<script lang="ts">
	import Card from './Card.svelte';
	import { savingsHeadline, savingsInternalNote, savingsOverlapNote } from '$lib/savingsView';
	import { decimalToNumber } from '$lib/chartShaping';
	import type { SavingsSummary } from '$lib/graphql/types';

	interface Props {
		summary: SavingsSummary | null;
	}
	let { summary }: Props = $props();

	const headline = $derived(summary ? savingsHeadline(summary) : null);
	const overlap = $derived(summary ? savingsOverlapNote(summary) : null);
	const internal = $derived(summary ? savingsInternalNote(summary) : null);

	const money = (v: string) =>
		decimalToNumber(v).toLocaleString('en-GB', {
			minimumFractionDigits: 2,
			maximumFractionDigits: 2
		});

	const toneClass: Record<string, string> = {
		good: 'text-income',
		bad: 'text-spending',
		neutral: 'text-slate-700'
	};
</script>

<Card title="Savings">
	{#if !summary}
		<p class="text-sm text-slate-500">
			No accounts are marked as savings yet. Mark one below and this figure appears.
		</p>
	{:else if headline}
		<div class="mb-4">
			<p class="text-sm text-slate-500">{headline.label}</p>
			<p class={`text-3xl font-semibold tabular-nums ${toneClass[headline.tone]}`}>
				{money(summary.netPutAside)}
				<span class="text-base font-normal text-slate-500">{summary.currency}</span>
			</p>
			<p class="mt-1 text-sm text-slate-500">{headline.detail}</p>
		</div>

		<table class="w-full text-sm">
			<thead>
				<tr class="border-b border-slate-200 text-left text-xs text-slate-500">
					<th scope="col" class="py-1.5 pr-3 font-medium">Account</th>
					<th scope="col" class="py-1.5 pr-3 text-right font-medium">In</th>
					<th scope="col" class="py-1.5 pr-3 text-right font-medium">Out</th>
					<th scope="col" class="py-1.5 text-right font-medium">Net</th>
				</tr>
			</thead>
			<tbody>
				{#each summary.accounts as flow (flow.account.id)}
					{@const net = decimalToNumber(flow.net)}
					<tr class="border-b border-slate-100 last:border-0">
						<td class="py-1.5 pr-3">{flow.account.displayName}</td>
						<td class="py-1.5 pr-3 text-right text-slate-600 tabular-nums">{money(flow.paidIn)}</td>
						<td class="py-1.5 pr-3 text-right text-slate-600 tabular-nums"
							>{money(flow.withdrawn)}</td
						>
						<td
							class={`py-1.5 text-right font-medium tabular-nums ${
								net > 0 ? 'text-income' : net < 0 ? 'text-spending' : 'text-slate-600'
							}`}>{money(flow.net)}</td
						>
					</tr>
				{/each}
			</tbody>
		</table>

		{#if overlap || internal}
			<div class="mt-3 space-y-1 border-t border-slate-200 pt-3">
				{#if overlap}
					<p class="text-xs text-slate-500" data-testid="savings-overlap-note">{overlap}</p>
				{/if}
				{#if internal}
					<p class="text-xs text-slate-500" data-testid="savings-internal-note">{internal}</p>
				{/if}
			</div>
		{/if}
	{/if}
</Card>
