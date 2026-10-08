<script lang="ts">
	/**
	 * `/admin/rules` (§6, §10 WP6): rule list grouped by state with an
	 * auto-approved badge, a recently-auto-approved section, inline
	 * create/edit via `RuleForm`, revoke and re-apply (reporting the
	 * re-queued count, §5).
	 *
	 * "Exempt from learning" lives here too: a rule row can stop the learner
	 * for its merchant, and the always-visible "Exempt from learning" card
	 * lists every exempt merchant with an undo, so the setting is never
	 * invisible. Exempting is confirmed in a dialog because it also removes
	 * the merchant's learned rules.
	 */
	import { invalidateAll } from '$app/navigation';
	import Button from '$lib/components/Button.svelte';
	import Badge from '$lib/components/Badge.svelte';
	import Card from '$lib/components/Card.svelte';
	import Modal from '$lib/components/Modal.svelte';
	import RuleForm from '$lib/components/RuleForm.svelte';
	import RulesTable from '$lib/components/admin/RulesTable.svelte';
	import { createGraphqlClient } from '$lib/graphqlClient';
	import {
		CREATE_RULE_MUTATION,
		EXEMPT_FROM_LEARNING_MUTATION,
		REAPPLY_RULE_MUTATION,
		REMOVE_LEARNING_EXEMPTION_MUTATION,
		SET_RULE_STATE_MUTATION,
		UPDATE_RULE_MUTATION
	} from '$lib/graphql/adminRulesQueries';
	import { describeExemptionImpact, sortByRecentlyCreated } from '$lib/rulesView';
	import type { LearningExemption, Rule, RuleInput } from '$lib/graphql/types';
	import type { PageData } from './$types';

	let { data }: { data: PageData } = $props();

	let formOpen = $state(false);
	let editingRule = $state<Rule | null>(null);
	let statusMessage = $state<string | null>(null);

	/** The merchant awaiting the user's confirmation to be exempted, if any. */
	let pendingExemption = $state<{ key: string; label: string } | null>(null);
	let exemptBusy = $state(false);
	let exemptError = $state('');
	let merchantDraft = $state('');

	const exemptKeys = $derived(new Set(data.exemptions.map((e) => e.counterpartyKey)));

	const recentlyAutoApproved = $derived(sortByRecentlyCreated(data.recentlyAutoApproved));

	function openCreate() {
		editingRule = null;
		formOpen = true;
	}

	function openEdit(rule: Rule) {
		editingRule = rule;
		formOpen = true;
	}

	function closeForm() {
		formOpen = false;
		editingRule = null;
	}

	async function submitForm(input: RuleInput) {
		const client = createGraphqlClient(fetch);
		if (editingRule) {
			await client.mutation(UPDATE_RULE_MUTATION, { id: editingRule.id, input }).toPromise();
		} else {
			await client.mutation(CREATE_RULE_MUTATION, { input }).toPromise();
		}
		closeForm();
		await invalidateAll();
	}

	async function revoke(rule: Rule) {
		const client = createGraphqlClient(fetch);
		await client.mutation(SET_RULE_STATE_MUTATION, { id: rule.id, state: 'REVOKED' }).toPromise();
		statusMessage = `"${rule.name}" revoked.`;
		await invalidateAll();
	}

	function askExempt(rule: Rule, key: string) {
		exemptError = '';
		pendingExemption = { key, label: rule.name.split(' → ')[0] || key };
	}

	function askExemptTyped(event: SubmitEvent) {
		event.preventDefault();
		const typed = merchantDraft.trim();
		if (!typed) return;
		exemptError = '';
		pendingExemption = { key: typed, label: typed };
	}

	async function confirmExempt() {
		if (!pendingExemption) return;
		exemptBusy = true;
		exemptError = '';
		try {
			const client = createGraphqlClient(fetch);
			const result = await client
				.mutation(EXEMPT_FROM_LEARNING_MUTATION, { counterpartyKey: pendingExemption.key })
				.toPromise();
			if (result.error) {
				exemptError = 'Could not exempt this merchant. Try again.';
				return;
			}
			statusMessage = `No more rules will be learned for "${pendingExemption.label}". Any rules learned for it were removed.`;
			pendingExemption = null;
			merchantDraft = '';
			await invalidateAll();
		} finally {
			exemptBusy = false;
		}
	}

	async function allowLearning(exemption: LearningExemption) {
		const client = createGraphqlClient(fetch);
		await client
			.mutation(REMOVE_LEARNING_EXEMPTION_MUTATION, { counterpartyKey: exemption.counterpartyKey })
			.toPromise();
		statusMessage = `Rules can be learned for "${exemption.displayName}" again.`;
		await invalidateAll();
	}

	async function reapply(rule: Rule) {
		const client = createGraphqlClient(fetch);
		const result = await client.mutation(REAPPLY_RULE_MUTATION, { id: rule.id }).toPromise();
		const count = result.data?.reapplyRule as number | undefined;
		statusMessage = `"${rule.name}" re-queued ${count ?? 0} transaction(s).`;
	}
</script>

<div class="flex flex-col gap-6">
	<div class="flex items-center justify-between">
		<h1 class="text-lg font-semibold text-slate-900">Rules</h1>
		<Button variant="primary" onclick={openCreate}>New rule</Button>
	</div>

	{#if data.error}
		<p class="text-sm text-red-600">Failed to load rules.</p>
	{/if}

	{#if statusMessage}
		<p class="rounded-md bg-slate-100 px-3 py-2 text-sm text-slate-700" role="status">
			{statusMessage}
		</p>
	{/if}

	<Card title="Exempt from learning">
		<p class="mb-3 text-sm text-slate-600">
			Rules are never learned for these merchants, so every transaction is labelled or split by hand
			(for example Amazon or PayPal, where one merchant covers many categories).
		</p>
		<ul class="flex flex-col divide-y divide-slate-100 text-sm" aria-label="Exempt merchants">
			{#each data.exemptions as exemption (exemption.counterpartyKey)}
				<li class="flex flex-wrap items-center justify-between gap-2 py-2">
					<div class="flex flex-col">
						<span class="flex items-center gap-2 font-medium text-slate-900">
							{exemption.displayName}
							<Badge text="always ask me" variant="warning" />
						</span>
						<span class="text-xs text-slate-500">
							{exemption.counterpartyKey} · {describeExemptionImpact(exemption.transactionCount)}
						</span>
					</div>
					<Button variant="ghost" onclick={() => allowLearning(exemption)}
						>Allow learning again</Button
					>
				</li>
			{:else}
				<li class="py-2 text-slate-500">
					No merchants are exempt. Use "Never learn this merchant" on a rule, or add one below.
				</li>
			{/each}
		</ul>
		<form class="mt-3 flex flex-wrap items-center gap-2" onsubmit={askExemptTyped}>
			<label class="sr-only" for="exempt-merchant">Merchant to exempt</label>
			<input
				id="exempt-merchant"
				class="min-w-0 flex-1 rounded-md border border-slate-300 px-3 py-1.5 text-sm"
				placeholder="Merchant name, e.g. Amazon"
				bind:value={merchantDraft}
			/>
			<Button type="submit" disabled={merchantDraft.trim() === ''}>Exempt merchant</Button>
		</form>
	</Card>

	{#if recentlyAutoApproved.length > 0}
		<Card title="Recently auto-approved">
			<ul class="flex flex-col gap-1 text-sm">
				{#each recentlyAutoApproved as rule (rule.id)}
					<li class="flex items-center justify-between">
						<span>{rule.name} → {rule.category.name}</span>
						<span class="text-xs text-slate-500">
							{rule.confidence !== null ? `${Math.round(rule.confidence * 100)}%` : ''}
						</span>
					</li>
				{/each}
			</ul>
		</Card>
	{/if}

	<Card>
		<RulesTable
			rules={data.rules}
			{exemptKeys}
			onedit={openEdit}
			onrevoke={revoke}
			onreapply={reapply}
			onexempt={askExempt}
		/>
	</Card>
</div>

<RuleForm
	open={formOpen}
	categories={data.categories}
	rule={editingRule}
	onsubmit={submitForm}
	onclose={closeForm}
/>

{#if pendingExemption}
	<Modal title="Never learn rules for this merchant?" onclose={() => (pendingExemption = null)}>
		<div class="flex flex-col gap-3 text-sm text-slate-700">
			<p>
				<strong>{pendingExemption.label}</strong> will be exempt from learning. Your decisions on it
				will no longer become rules, so each transaction needs a label (or a split) from you.
			</p>
			<ul class="list-inside list-disc text-slate-600">
				<li>
					Rules learned for this merchant are removed. Their transactions fall back to the next
					label source.
				</li>
				<li>Rules you created, edited or approved yourself are kept.</li>
				<li>You can undo this any time under "Exempt from learning".</li>
			</ul>
			{#if exemptError}
				<p class="text-red-600" role="alert">{exemptError}</p>
			{/if}
			<div class="flex justify-end gap-2">
				<Button variant="ghost" onclick={() => (pendingExemption = null)}>Cancel</Button>
				<Button variant="primary" disabled={exemptBusy} onclick={confirmExempt}>
					Exempt merchant
				</Button>
			</div>
		</div>
	</Modal>
{/if}
