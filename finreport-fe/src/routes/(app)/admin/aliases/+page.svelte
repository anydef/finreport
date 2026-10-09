<script lang="ts">
	/**
	 * `/admin/aliases`: the user's nicknames for merchants and for their own
	 * accounts. One merchant alias covers every spelling of that merchant, and
	 * also covers an own account that is not connected here (it only ever
	 * appears as a counterparty on a connected account's transactions). Display
	 * only: the bank's name is always shown alongside, and removing an alias
	 * brings it back everywhere.
	 */
	import { invalidateAll } from '$app/navigation';
	import Badge from '$lib/components/Badge.svelte';
	import Button from '$lib/components/Button.svelte';
	import Card from '$lib/components/Card.svelte';
	import Field from '$lib/components/Field.svelte';
	import Modal from '$lib/components/Modal.svelte';
	import { createGraphqlClient } from '$lib/graphqlClient';
	import {
		REMOVE_DISPLAY_ALIAS_MUTATION,
		SET_DISPLAY_ALIAS_MUTATION
	} from '$lib/graphql/adminAliasesQueries';
	import {
		MAX_ALIAS_LENGTH,
		accountOptionLabel,
		aliasError,
		describeAliasTarget,
		kindLabel,
		sortAliases
	} from '$lib/aliasView';
	import type { DisplayAlias, DisplayAliasKind } from '$lib/graphql/types';
	import type { PageData } from './$types';

	let { data }: { data: PageData } = $props();

	const aliases = $derived(sortAliases(data.aliases));

	let statusMessage = $state<string | null>(null);

	// Add form
	let newKind = $state<DisplayAliasKind>('COUNTERPARTY');
	let newMerchant = $state('');
	let newAccountId = $state('');
	let newAlias = $state('');
	let addError = $state('');
	let addBusy = $state(false);

	// Edit dialog
	let editing = $state<DisplayAlias | null>(null);
	let editAlias = $state('');
	let editError = $state('');
	let editBusy = $state(false);

	const newKey = $derived(newKind === 'ACCOUNT' ? newAccountId : newMerchant.trim());

	async function save(kind: DisplayAliasKind, key: string, alias: string): Promise<string | null> {
		const client = createGraphqlClient(fetch);
		const result = await client
			.mutation(SET_DISPLAY_ALIAS_MUTATION, { kind, key, alias: alias.trim() })
			.toPromise();
		if (result.error) return result.error.graphQLErrors[0]?.message ?? 'Could not save. Try again.';
		await invalidateAll();
		return null;
	}

	async function add(event: SubmitEvent) {
		event.preventDefault();
		const problem = aliasError(newAlias);
		if (problem) {
			addError = problem;
			return;
		}
		if (newKey === '') {
			addError = newKind === 'ACCOUNT' ? 'Pick an account.' : 'Enter the merchant name.';
			return;
		}
		addBusy = true;
		addError = '';
		const error = await save(newKind, newKey, newAlias);
		addBusy = false;
		if (error) {
			addError = error;
			return;
		}
		statusMessage = `"${newAlias.trim()}" saved.`;
		newMerchant = '';
		newAlias = '';
	}

	function openEdit(alias: DisplayAlias) {
		editing = alias;
		editAlias = alias.alias;
		editError = '';
	}

	async function confirmEdit(event: SubmitEvent) {
		event.preventDefault();
		if (!editing) return;
		const problem = aliasError(editAlias);
		if (problem) {
			editError = problem;
			return;
		}
		editBusy = true;
		const error = await save(editing.kind, editing.key, editAlias);
		editBusy = false;
		if (error) {
			editError = error;
			return;
		}
		statusMessage = `"${editAlias.trim()}" saved.`;
		editing = null;
	}

	async function remove(alias: DisplayAlias) {
		const client = createGraphqlClient(fetch);
		const result = await client
			.mutation(REMOVE_DISPLAY_ALIAS_MUTATION, { kind: alias.kind, key: alias.key })
			.toPromise();
		statusMessage = result.error
			? 'Could not remove the nickname. Try again.'
			: `"${alias.alias}" removed; "${alias.rawName}" shows again.`;
		await invalidateAll();
	}
</script>

<div class="flex flex-col gap-6">
	<h1 class="text-lg font-semibold text-slate-900">Nicknames</h1>

	{#if data.error}
		<p class="text-sm text-red-600">Failed to load nicknames.</p>
	{/if}

	{#if statusMessage}
		<p class="rounded-md bg-slate-100 px-3 py-2 text-sm text-slate-700" role="status">
			{statusMessage}
		</p>
	{/if}

	<Card title="Your nicknames">
		<p class="mb-3 text-sm text-slate-600">
			Give a merchant or one of your accounts a name you recognise. One merchant nickname covers
			every spelling of that merchant, and also an account of yours that is not connected here. The
			bank's own name is never changed and always shown alongside.
		</p>
		<ul class="flex flex-col divide-y divide-slate-100 text-sm" aria-label="Nicknames">
			{#each aliases as alias (alias.kind + ':' + alias.key)}
				<li class="flex flex-wrap items-center justify-between gap-2 py-2">
					<div class="flex flex-col">
						<span class="flex items-center gap-2 font-medium text-slate-900">
							{alias.alias}
							<Badge text={kindLabel(alias.kind)} variant={alias.kind === 'ACCOUNT' ? 'info' : 'neutral'} />
						</span>
						<span class="text-xs text-slate-500">{describeAliasTarget(alias)}</span>
					</div>
					<div class="flex gap-1">
						<Button variant="ghost" onclick={() => openEdit(alias)}>Edit</Button>
						<Button variant="ghost" onclick={() => remove(alias)}>Remove</Button>
					</div>
				</li>
			{:else}
				<li class="py-2 text-slate-500">
					No nicknames yet. Add one below, for example "Mum" for a transfer you send every month.
				</li>
			{/each}
		</ul>
	</Card>

	<Card title="Add a nickname">
		<form class="flex flex-col gap-3" onsubmit={add}>
			<Field label="What to rename" for="alias-kind">
				<select
					id="alias-kind"
					class="rounded-md border border-slate-300 px-3 py-1.5 text-sm"
					bind:value={newKind}
				>
					<option value="COUNTERPARTY">A merchant or someone I pay</option>
					<option value="ACCOUNT">One of my connected accounts</option>
				</select>
			</Field>
			{#if newKind === 'ACCOUNT'}
				<Field label="Account" for="alias-account">
					<select
						id="alias-account"
						class="rounded-md border border-slate-300 px-3 py-1.5 text-sm"
						bind:value={newAccountId}
					>
						<option value="">Choose an account</option>
						{#each data.accounts as account (account.id)}
							<option value={account.id}>{accountOptionLabel(account)}</option>
						{/each}
					</select>
				</Field>
			{:else}
				<Field label="Name as it appears on a transaction" for="alias-merchant">
					<input
						id="alias-merchant"
						class="rounded-md border border-slate-300 px-3 py-1.5 text-sm"
						placeholder="e.g. Amazon Payments Europe"
						bind:value={newMerchant}
					/>
				</Field>
			{/if}
			<Field label="Nickname" for="alias-name">
				<input
					id="alias-name"
					class="rounded-md border border-slate-300 px-3 py-1.5 text-sm"
					maxlength={MAX_ALIAS_LENGTH}
					placeholder="e.g. Mum"
					bind:value={newAlias}
				/>
			</Field>
			{#if addError}
				<p class="text-sm text-red-600" role="alert">{addError}</p>
			{/if}
			<div>
				<Button type="submit" variant="primary" disabled={addBusy}>Save nickname</Button>
			</div>
		</form>
	</Card>
</div>

{#if editing}
	<Modal title="Edit nickname" onclose={() => (editing = null)}>
		<form class="flex flex-col gap-3 text-sm text-slate-700" onsubmit={confirmEdit}>
			<p class="text-slate-600">{describeAliasTarget(editing)}</p>
			<Field label="Nickname" for="alias-edit">
				<input
					id="alias-edit"
					class="rounded-md border border-slate-300 px-3 py-1.5 text-sm"
					maxlength={MAX_ALIAS_LENGTH}
					bind:value={editAlias}
				/>
			</Field>
			{#if editError}
				<p class="text-red-600" role="alert">{editError}</p>
			{/if}
			<div class="flex justify-end gap-2">
				<Button type="button" variant="ghost" onclick={() => (editing = null)}>Cancel</Button>
				<Button type="submit" variant="primary" disabled={editBusy}>Save</Button>
			</div>
		</form>
	</Modal>
{/if}
