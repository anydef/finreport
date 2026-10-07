<script lang="ts">
	/**
	 * `/admin/rules` (§6, §10 WP6): rule list grouped by state with an
	 * auto-approved badge, a recently-auto-approved section, inline
	 * create/edit via `RuleForm`, revoke and re-apply (reporting the
	 * re-queued count, §5).
	 */
	import { invalidateAll } from '$app/navigation';
	import Button from '$lib/components/Button.svelte';
	import Card from '$lib/components/Card.svelte';
	import RuleForm from '$lib/components/RuleForm.svelte';
	import RulesTable from '$lib/components/admin/RulesTable.svelte';
	import { createGraphqlClient } from '$lib/graphqlClient';
	import {
		CREATE_RULE_MUTATION,
		REAPPLY_RULE_MUTATION,
		SET_RULE_STATE_MUTATION,
		UPDATE_RULE_MUTATION
	} from '$lib/graphql/adminRulesQueries';
	import { sortByRecentlyCreated } from '$lib/rulesView';
	import type { Rule, RuleInput } from '$lib/graphql/types';
	import type { PageData } from './$types';

	let { data }: { data: PageData } = $props();

	let formOpen = $state(false);
	let editingRule = $state<Rule | null>(null);
	let statusMessage = $state<string | null>(null);

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
		<RulesTable rules={data.rules} onedit={openEdit} onrevoke={revoke} onreapply={reapply} />
	</Card>
</div>

<RuleForm
	open={formOpen}
	categories={data.categories}
	rule={editingRule}
	onsubmit={submitForm}
	onclose={closeForm}
/>
