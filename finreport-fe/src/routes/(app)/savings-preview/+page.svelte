<!--
  PREVIEW ONLY. Nothing here talks to the backend.

  `isSavings`, `savingsSummary` and `setAccountSavings` do not exist in
  `schema.graphql` yet, and `queries.test.ts` validates every GraphQL
  operation against that schema - so inventing them there would (rightly)
  fail. This page therefore reads `graphql/savingsMock` directly, which keeps
  the API contract honest while the shape of the card is reviewed.

  Delete this route when the resolver lands; the card component itself is
  meant to survive.
-->
<script lang="ts">
	import Card from '$lib/components/Card.svelte';
	import SavingsCard from '$lib/components/SavingsCard.svelte';
	import {
		MOCK_SAVINGS_ACCOUNTS,
		TOGGLEABLE_CURRENT_ACCOUNTS,
		isSavingsAccount,
		setSavingsAccount,
		mockSavingsSummary
	} from '$lib/graphql/savingsMock';
	import type { SavingsSummary } from '$lib/graphql/types';

	const rows = [
		...TOGGLEABLE_CURRENT_ACCOUNTS.map((a) => ({
			id: a.id,
			displayName: a.displayName,
			accountType: a.accountType
		})),
		...MOCK_SAVINGS_ACCOUNTS.map((a) => ({
			id: a.id,
			displayName: a.displayName,
			accountType: a.accountType
		}))
	];

	// The mock keeps the flags in module state, so a toggle is not itself
	// reactive; bumping this is what recomputes both the checkboxes and the
	// card from the mock's own flows.
	let generation = $state(0);
	const flags = $derived.by(() => {
		void generation;
		return Object.fromEntries(rows.map((r) => [r.id, isSavingsAccount(r.id)]));
	});
	const summary = $derived.by(() => {
		void generation;
		return (mockSavingsSummary() as { savingsSummary: SavingsSummary | null }).savingsSummary;
	});

	function toggle(id: string) {
		setSavingsAccount(id, !isSavingsAccount(id));
		generation += 1;
	}
</script>

<div class="space-y-6">
	<div class="rounded-md border border-amber-300 bg-amber-50 px-4 py-3">
		<p class="text-sm font-medium text-amber-900">Preview — not connected to your data</p>
		<p class="mt-1 text-sm text-amber-800">
			These figures come from sample movements, not your accounts. Toggling an account below
			recomputes the card so the behaviour can be checked before the backend is built.
		</p>
	</div>

	<Card title="Which accounts are savings?">
		<p class="mb-3 text-sm text-slate-500">
			Marking an account as savings changes reporting only. It never moves money, and it does not
			affect how transactions are categorised.
		</p>
		<ul class="divide-y divide-slate-100">
			{#each rows as account (account.id)}
				<li class="flex items-center justify-between py-2">
					<span>
						<span class="text-sm text-slate-800">{account.displayName}</span>
						{#if account.accountType}
							<span class="ml-2 text-xs text-slate-500">{account.accountType}</span>
						{/if}
					</span>
					<label class="flex cursor-pointer items-center gap-2 text-sm">
						<input
							type="checkbox"
							checked={flags[account.id]}
							onchange={() => toggle(account.id)}
							class="accent-brand h-4 w-4"
							aria-label={`Treat ${account.displayName} as a savings account`}
						/>
						<span class="text-slate-600">Savings</span>
					</label>
				</li>
			{/each}
		</ul>
	</Card>

	<SavingsCard {summary} />
</div>
