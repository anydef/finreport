<script lang="ts">
	import { formatAmount, formatDisplayDate } from '$lib/format';
	import { labelSourceBadge, needsReviewBadge } from '$lib/labelBadge';
	import Badge from './Badge.svelte';
	import TagEditor from './TagEditor.svelte';
	import RecurringBadge from './RecurringBadge.svelte';
	import type { Transaction } from '$lib/graphql/types';

	interface Props {
		transactions: Transaction[];
		currency: string;
		/** Omitted = tags/recurring are read-only (no mutation wiring, e.g. a narrow preview table). */
		onSetTags?: (transactionId: string, tags: string[]) => void | Promise<void>;
		onSetRecurring?: (transactionId: string, recurring: boolean | null) => void | Promise<void>;
	}

	let { transactions, currency, onSetTags, onSetRecurring }: Props = $props();
</script>

{#if transactions.length === 0}
	<p class="py-8 text-center text-sm text-slate-500">No transactions in this period.</p>
{:else}
	<div class="overflow-x-auto">
		<table class="w-full text-sm">
			<thead>
				<tr class="border-b border-slate-200 text-left text-slate-500">
					<th class="py-2 pr-4 font-medium">Date</th>
					<th class="py-2 pr-4 font-medium">Counterparty</th>
					<th class="py-2 pr-4 font-medium">Description</th>
					<th class="py-2 pr-4 font-medium">Category</th>
					<th class="py-2 pr-4 font-medium">Tags</th>
					<th class="py-2 pr-4 font-medium">Flags</th>
					<th class="py-2 pr-4 text-right font-medium">Amount</th>
				</tr>
			</thead>
			<tbody>
				{#each transactions as tx (tx.id)}
					<tr class="border-b border-slate-100 last:border-0 hover:bg-slate-50">
						<td class="py-2 pr-4 whitespace-nowrap text-slate-600"
							>{formatDisplayDate(tx.bookingDate)}</td
						>
						<td class="py-2 pr-4">{tx.counterpartyName ?? 'Unknown'}</td>
						<td class="py-2 pr-4 text-slate-600">{tx.description ?? ''}</td>
						<td class="py-2 pr-4">
							<div class="flex flex-wrap items-center gap-1">
								<span class="text-slate-700">{tx.label?.category?.name ?? '—'}</span>
								{#if labelSourceBadge(tx.label)}
									{@const badge = labelSourceBadge(tx.label)!}
									<Badge text={badge.text} variant={badge.variant} />
								{/if}
								{#if needsReviewBadge(tx.label)}
									{@const pill = needsReviewBadge(tx.label)!}
									<a href="/review" class="inline-block">
										<Badge text={pill.text} variant={pill.variant} />
									</a>
								{/if}
							</div>
						</td>
						<td class="py-2 pr-4">
							{#if onSetTags}
								<TagEditor tags={tx.tags} onSave={(tags) => onSetTags(tx.id, tags)} />
							{:else}
								<div class="flex flex-wrap gap-1">
									{#each tx.tags as tag (tag)}
										<Badge text={tag} variant="neutral" />
									{/each}
								</div>
							{/if}
						</td>
						<td class="py-2 pr-4">
							<div class="flex flex-wrap items-center gap-1">
								{#if tx.transfer}
									<Badge text="⇄ transfer" variant="info" />
								{/if}
								{#if onSetRecurring}
									<RecurringBadge
										recurring={tx.recurring}
										onToggle={(next) => onSetRecurring(tx.id, next)}
									/>
								{:else if tx.recurring.isRecurring}
									<Badge
										text={tx.recurring.source === 'USER'
											? '↻ recurring · you'
											: '↻ recurring · auto'}
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
				{/each}
			</tbody>
		</table>
	</div>
{/if}
