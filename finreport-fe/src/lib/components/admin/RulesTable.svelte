<script lang="ts">
	/**
	 * `/admin/rules` list (§6, §10 WP6): grouped by state, an auto-approved
	 * badge for learned rules published above the threshold (§2.8), and the
	 * actions revoke / re-apply / edit.
	 */
	import Badge from '../Badge.svelte';
	import Button from '../Button.svelte';
	import {
		describeReach,
		groupRulesByState,
		isAutoApproved,
		merchantKeyOf,
		summarizeConditions
	} from '$lib/rulesView';
	import type { Rule } from '$lib/graphql/types';

	interface Props {
		rules: Rule[];
		onedit: (rule: Rule) => void;
		onrevoke: (rule: Rule) => void;
		onreapply: (rule: Rule) => void;
		/** Merchant keys currently exempt from learning (their rows show it instead of the action). */
		exemptKeys?: ReadonlySet<string>;
		/** "Never learn rules for this merchant again": asks the page to confirm and exempt the key. */
		onexempt?: (rule: Rule, counterpartyKey: string) => void;
	}

	let { rules, onedit, onrevoke, onreapply, exemptKeys = new Set(), onexempt }: Props = $props();

	const groups = $derived(groupRulesByState(rules));

	const sections: { title: string; key: keyof ReturnType<typeof groupRulesByState> }[] = [
		{ title: 'Active', key: 'active' },
		{ title: 'In review', key: 'inReview' },
		{ title: 'Revoked', key: 'revoked' },
		{ title: 'Rejected', key: 'rejected' }
	];
</script>

{#snippet badge(text: string, tone: 'brand' | 'muted')}
	<span
		class={`rounded-full px-2 py-0.5 text-xs font-medium ${
			tone === 'brand' ? 'bg-brand/10 text-brand' : 'bg-slate-100 text-slate-600'
		}`}
	>
		{text}
	</span>
{/snippet}

{#snippet ruleRow(rule: Rule)}
	<tr class="border-b border-slate-100 align-top last:border-0 hover:bg-slate-50">
		<td class="py-2 pr-4">
			<div class="font-medium text-slate-900">{rule.name}</div>
			<div class="mt-1 flex flex-wrap gap-1">
				{@render badge(rule.origin === 'LEARNED' ? 'learned' : 'user', 'muted')}
				{#if isAutoApproved(rule)}
					{@render badge('auto-approved', 'brand')}
				{/if}
				{#if merchantKeyOf(rule) && exemptKeys.has(merchantKeyOf(rule) ?? '')}
					<Badge text="merchant exempt from learning" variant="warning" />
				{/if}
			</div>
		</td>
		<td class="py-2 pr-4 text-slate-600">{rule.category.name}</td>
		<td class="py-2 pr-4 text-slate-600">
			<ul class="list-inside list-disc">
				{#each summarizeConditions(rule.conditions) as line (line)}
					<li>{line}</li>
				{:else}
					<li class="list-none text-slate-400">no conditions</li>
				{/each}
			</ul>
		</td>
		<td class="py-2 pr-4 text-slate-600">
			{rule.confidence !== null ? `${Math.round(rule.confidence * 100)}%` : '—'}
			<span class="block text-xs text-slate-400">{rule.evidenceCount} evidence</span>
		</td>
		<td
			class={`py-2 pr-4 whitespace-nowrap ${rule.matchingTransactionCount === 0 ? 'text-amber-600' : 'text-slate-600'}`}
			title="Transactions in your accounts these conditions match, whatever their current label"
		>
			{describeReach(rule.matchingTransactionCount)}
		</td>
		<td class="py-2 pr-0 text-right whitespace-nowrap">
			<Button variant="ghost" onclick={() => onedit(rule)}>Edit</Button>
			{#if rule.state !== 'REVOKED'}
				<Button variant="ghost" onclick={() => onrevoke(rule)}>Revoke</Button>
			{/if}
			<Button variant="ghost" onclick={() => onreapply(rule)}>Re-apply</Button>
			{#if onexempt}
				{@const merchant = merchantKeyOf(rule)}
				{#if merchant && !exemptKeys.has(merchant)}
					<Button
						variant="ghost"
						title="Stop learning rules for this merchant, so every transaction is labelled or split by hand"
						onclick={() => onexempt(rule, merchant)}>Never learn this merchant</Button
					>
				{/if}
			{/if}
		</td>
	</tr>
{/snippet}

<div class="flex flex-col gap-6">
	{#each sections as section (section.key)}
		{@const sectionRules = groups[section.key]}
		<div>
			<h3 class="mb-2 text-sm font-semibold text-slate-500 uppercase">
				{section.title} ({sectionRules.length})
			</h3>
			{#if sectionRules.length === 0}
				<p class="text-sm text-slate-400">None.</p>
			{:else}
				<div class="overflow-x-auto">
					<table class="w-full text-sm">
						<thead>
							<tr class="border-b border-slate-200 text-left text-slate-500">
								<th class="py-2 pr-4 font-medium">Rule</th>
								<th class="py-2 pr-4 font-medium">Category</th>
								<th class="py-2 pr-4 font-medium">Match conditions</th>
								<th class="py-2 pr-4 font-medium">Confidence</th>
								<th class="py-2 pr-4 font-medium">Matches</th>
								<th class="py-2 pr-0 text-right font-medium">Actions</th>
							</tr>
						</thead>
						<tbody>
							{#each sectionRules as rule (rule.id)}
								{@render ruleRow(rule)}
							{/each}
						</tbody>
					</table>
				</div>
			{/if}
		</div>
	{/each}
</div>
