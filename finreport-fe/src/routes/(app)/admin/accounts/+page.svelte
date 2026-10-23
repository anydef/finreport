<script lang="ts">
	/**
	 * `/admin/accounts`: which of the user's accounts count as savings. A
	 * reporting choice, per user, like a nickname: it never moves money and
	 * does not touch categorisation. Refetches after each toggle (as
	 * `/admin/aliases` does) so the list always shows what the server holds.
	 */
	import { invalidateAll } from '$app/navigation';
	import Card from '$lib/components/Card.svelte';
	import { createGraphqlClient } from '$lib/graphqlClient';
	import { SET_ACCOUNT_SAVINGS_MUTATION } from '$lib/graphql/queries';
	import type { Account } from '$lib/graphql/types';
	import type { PageData } from './$types';

	let { data }: { data: PageData } = $props();

	let statusMessage = $state<string | null>(null);
	let errorMessage = $state<string | null>(null);
	let busyId = $state<string | null>(null);

	async function toggle(account: Account, isSavings: boolean) {
		busyId = account.id;
		errorMessage = null;
		statusMessage = null;
		const result = await createGraphqlClient(fetch)
			.mutation(SET_ACCOUNT_SAVINGS_MUTATION, { accountId: account.id, isSavings })
			.toPromise();
		if (result.error) {
			errorMessage =
				result.error.graphQLErrors[0]?.message ?? 'Could not save the change. Try again.';
		} else {
			statusMessage = isSavings
				? `${account.displayName} is now counted as savings.`
				: `${account.displayName} is no longer counted as savings.`;
		}
		await invalidateAll();
		busyId = null;
	}
</script>

<div class="flex flex-col gap-6">
	<h1 class="text-lg font-semibold text-slate-900">Accounts</h1>

	{#if data.error}
		<p class="text-sm text-red-600">Failed to load accounts.</p>
	{/if}

	{#if errorMessage}
		<p class="text-sm text-red-600" role="alert">{errorMessage}</p>
	{/if}

	{#if statusMessage}
		<p class="rounded-md bg-slate-100 px-3 py-2 text-sm text-slate-700" role="status">
			{statusMessage}
		</p>
	{/if}

	<Card title="Which accounts are savings?">
		<p class="mb-3 text-sm text-slate-600">
			Marking an account as savings changes reporting only. It never moves money, and it does not
			affect how transactions are categorised. The dashboard's Savings card adds up the change in
			the accounts you mark here.
		</p>
		<ul class="divide-y divide-slate-100" aria-label="Accounts">
			{#each data.accounts as account (account.id)}
				<li class="flex items-center justify-between gap-2 py-2">
					<span>
						<span class="text-sm text-slate-800">{account.displayName}</span>
						{#if account.accountType}
							<span class="ml-2 text-xs text-slate-500">{account.accountType}</span>
						{/if}
					</span>
					<label class="flex cursor-pointer items-center gap-2 text-sm">
						<input
							type="checkbox"
							checked={account.isSavings}
							disabled={busyId === account.id}
							onchange={(e) => toggle(account, e.currentTarget.checked)}
							class="accent-brand h-4 w-4"
							aria-label={`Treat ${account.displayName} as a savings account`}
						/>
						<span class="text-slate-600">Savings</span>
					</label>
				</li>
			{:else}
				<li class="py-2 text-sm text-slate-500">No accounts yet.</li>
			{/each}
		</ul>
	</Card>
</div>
