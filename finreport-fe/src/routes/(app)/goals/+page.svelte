<script lang="ts">
	import { goto, invalidateAll } from '$app/navigation';
	import { page } from '$app/state';
	import { createGraphqlClient } from '$lib/graphqlClient';
	import { CREATE_GOAL_MUTATION } from '$lib/graphql/queries';
	import type { GoalInput } from '$lib/graphql/types';
	import Button from '$lib/components/Button.svelte';
	import Card from '$lib/components/Card.svelte';
	import GoalCard from '$lib/components/GoalCard.svelte';
	import GoalForm from '$lib/components/GoalForm.svelte';
	import Modal from '$lib/components/Modal.svelte';
	import type { PageData } from './$types';

	let { data }: { data: PageData } = $props();

	let adding = $state(false);
	let busy = $state(false);
	let formError = $state('');

	async function createGoal(input: GoalInput) {
		busy = true;
		formError = '';
		try {
			const result = await createGraphqlClient(fetch)
				.mutation(CREATE_GOAL_MUTATION, { input })
				.toPromise();
			if (result.error) throw new Error(result.error.message);
			adding = false;
			await invalidateAll();
		} catch (err) {
			formError = err instanceof Error ? err.message : 'Failed to create the goal';
		} finally {
			busy = false;
		}
	}

	function closeForm() {
		adding = false;
		formError = '';
	}

	function toggleArchived() {
		const params = new URLSearchParams(page.url.searchParams);
		if (data.includeArchived) params.delete('archived');
		else params.set('archived', 'true');
		const query = params.toString();
		goto(query ? `?${query}` : page.url.pathname, { keepFocus: true, noScroll: true });
	}
</script>

<svelte:head>
	<title>Goals · finreport</title>
</svelte:head>

<div class="flex flex-col gap-6">
	<div class="flex items-center justify-between">
		<h1 class="text-lg font-semibold text-slate-900">Goals</h1>
		<div class="flex gap-2">
			<Button variant="ghost" onclick={toggleArchived}>
				{data.includeArchived ? 'Hide archived' : 'Show archived'}
			</Button>
			<Button variant="primary" onclick={() => (adding = true)}>Add goal</Button>
		</div>
	</div>

	{#if data.error}
		<p role="alert" class="text-sm text-[var(--color-spending)]">
			Failed to load goals from the GraphQL API.
		</p>
	{:else if data.goals.length === 0}
		<Card>
			<p class="py-8 text-center text-sm text-slate-500">
				No goals yet. Add a spending limit or a saving target to start tracking.
			</p>
		</Card>
	{:else}
		<div class="grid grid-cols-1 gap-4 md:grid-cols-2 lg:grid-cols-3">
			{#each data.goals as goal (goal.id)}
				<GoalCard {goal} progress={data.progress[goal.id]} />
			{/each}
		</div>
	{/if}
</div>

{#if adding}
	<Modal title="Add goal" onclose={closeForm}>
		<GoalForm
			categories={data.categories}
			{busy}
			error={formError}
			onsubmit={createGoal}
			oncancel={closeForm}
		/>
	</Modal>
{/if}
