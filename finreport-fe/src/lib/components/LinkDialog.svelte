<script lang="ts">
	/**
	 * Create, edit or remove a reimbursement link. The dialog holds the link's
	 * members (money out on the expense side, money in on the offsetting side),
	 * shows what the link would come to before it is saved (a partial
	 * reimbursement is stated as partial, more back than paid as a surplus),
	 * and suggests counterparts for the anchor transaction: opposite sign,
	 * similar amount, nearby date, not already linked. Typing a search widens
	 * the suggestions to the whole history.
	 *
	 * Nothing here rewrites a period: a link only annotates, and removing it
	 * puts the view back exactly as it was.
	 */
	import { untrack } from 'svelte';
	import Modal from './Modal.svelte';
	import Button from './Button.svelte';
	import { formatDisplayDate } from '$lib/format';
	import { createGraphqlClient } from '$lib/graphqlClient';
	import {
		CREATE_TRANSACTION_LINK_MUTATION,
		LINK_CANDIDATES_QUERY,
		REMOVE_TRANSACTION_LINK_MUTATION,
		UPDATE_TRANSACTION_LINK_MUTATION
	} from '$lib/graphql/queries';
	import type { LinkCandidate, TransactionLink } from '$lib/graphql/types';
	import {
		dialogMemberFromTransaction,
		formatMagnitude,
		linkInput,
		previewOffsets,
		saveBlocker,
		statusLabel,
		type DialogMember
	} from '$lib/reimbursement';

	interface Props {
		/** The transaction suggestions are found for. */
		anchor: DialogMember;
		/** Where the dialog starts: the anchor alone, the ticked rows, or an existing link's members. */
		initial: DialogMember[];
		/** Set when editing; enables "Remove link". */
		link?: TransactionLink | null;
		/** Called after a save or a removal, so the page can reload. */
		ondone: () => void;
		onclose: () => void;
	}

	let { anchor, initial, link = null, ondone, onclose }: Props = $props();

	let members = $state<DialogMember[]>(untrack(() => untrackedCopy(initial)));
	let note = $state(untrack(() => link?.note ?? ''));
	let search = $state('');
	let candidates = $state<LinkCandidate[] | null>(null);
	let busy = $state(false);
	let errorMessage = $state('');

	function untrackedCopy(list: DialogMember[]): DialogMember[] {
		return list.map((m) => ({ ...m }));
	}

	const client = () => createGraphqlClient(fetch);

	async function loadCandidates() {
		const result = await client()
			.query(LINK_CANDIDATES_QUERY, {
				transactionId: anchor.id,
				search: search.trim() === '' ? null : search.trim(),
				limit: 8
			})
			.toPromise();
		candidates = result.error
			? []
			: ((result.data?.linkCandidates ?? []) as LinkCandidate[]).filter(
					(c) => !members.some((m) => m.id === c.transaction.id)
				);
	}

	// Once, on open; later searches and removals call it explicitly.
	$effect(() => {
		untrack(loadCandidates);
	});

	const expenses = $derived(members.filter((m) => m.role === 'EXPENSE'));
	const offsets = $derived(members.filter((m) => m.role === 'OFFSET'));
	const currency = $derived(members[0]?.currency ?? anchor.currency);
	const preview = $derived(
		previewOffsets(
			expenses.map((m) => m.amount),
			offsets.map((m) => m.amount)
		)
	);
	const blocker = $derived(saveBlocker(members));

	function add(candidate: LinkCandidate) {
		members = [...members, dialogMemberFromTransaction(candidate.transaction)];
		candidates = (candidates ?? []).filter((c) => c.transaction.id !== candidate.transaction.id);
	}

	function drop(id: string) {
		members = members.filter((m) => m.id !== id);
		loadCandidates();
	}

	async function run<T>(query: string, variables: Record<string, unknown>, field: string) {
		busy = true;
		errorMessage = '';
		try {
			const result = await client().mutation(query, variables).toPromise();
			if (result.error || result.data?.[field] === undefined || result.data?.[field] === null) {
				throw new Error(result.error?.message ?? 'The link could not be saved.');
			}
			return result.data[field] as T;
		} catch (err) {
			errorMessage = err instanceof Error ? err.message : 'The link could not be saved.';
			return undefined;
		} finally {
			busy = false;
		}
	}

	async function save() {
		if (blocker) return;
		const input = linkInput(members, note);
		const saved = link
			? await run(UPDATE_TRANSACTION_LINK_MUTATION, { id: link.id, input }, 'updateTransactionLink')
			: await run(CREATE_TRANSACTION_LINK_MUTATION, { input }, 'createTransactionLink');
		if (saved) ondone();
	}

	async function unlink() {
		if (!link) return;
		const removed = await run<boolean>(
			REMOVE_TRANSACTION_LINK_MUTATION,
			{ id: link.id },
			'removeTransactionLink'
		);
		if (removed !== undefined) ondone();
	}

	const input =
		'rounded-md border border-slate-300 px-2 py-1.5 text-sm focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-brand';
</script>

{#snippet memberRow(m: DialogMember)}
	<li class="flex items-center justify-between gap-2 text-sm">
		<span class="min-w-0 truncate">
			<span class="font-medium">{m.label}</span>
			{#if m.bookingDate}<span class="text-slate-500">, {formatDisplayDate(m.bookingDate)}</span
				>{/if}
		</span>
		<span class="flex shrink-0 items-center gap-2">
			<span class="tabular-nums">{formatMagnitude(m.amount, m.currency)}</span>
			<button
				type="button"
				onclick={() => drop(m.id)}
				aria-label="Take {m.label} out of the link"
				class="focus-visible:outline-brand rounded px-1.5 text-slate-500 hover:bg-slate-100 focus-visible:outline focus-visible:outline-2"
			>
				✕
			</button>
		</span>
	</li>
{/snippet}

<Modal title={link ? 'Edit reimbursement link' : 'Link a reimbursement'} {onclose} class="max-w-xl">
	<p class="text-xs text-slate-500">
		A link only annotates. Both transactions keep their own month and amount; removing the link
		restores the view exactly.
	</p>

	<section aria-label="Paid" class="flex flex-col gap-1">
		<h3 class="text-xs font-semibold tracking-wide text-slate-500 uppercase">Paid out</h3>
		{#if expenses.length === 0}
			<p class="text-sm text-slate-400">Nothing yet. Pick the expense below.</p>
		{:else}
			<ul class="flex flex-col gap-1">
				{#each expenses as m (m.id)}{@render memberRow(m)}{/each}
			</ul>
		{/if}
	</section>
	<section aria-label="Paid back" class="flex flex-col gap-1">
		<h3 class="text-xs font-semibold tracking-wide text-slate-500 uppercase">Paid back</h3>
		{#if offsets.length === 0}
			<p class="text-sm text-slate-400">Nothing yet. Pick the reimbursement below.</p>
		{:else}
			<ul class="flex flex-col gap-1">
				{#each offsets as m (m.id)}{@render memberRow(m)}{/each}
			</ul>
		{/if}
	</section>

	<p
		role="status"
		data-testid="link-preview"
		class="rounded-md border px-3 py-2 text-sm {preview.status === 'PARTIAL'
			? 'border-amber-400 bg-amber-50 text-amber-900'
			: preview.status === 'INCOMPLETE'
				? 'border-slate-200 bg-slate-50 text-slate-600'
				: 'border-emerald-300 bg-emerald-50 text-emerald-900'}"
	>
		<strong>{statusLabel(preview.status)}.</strong>
		{#if preview.status !== 'INCOMPLETE'}
			{formatMagnitude(preview.expenseTotal, currency)} paid,
			{formatMagnitude(preview.reimbursed, currency)} reimbursed,
			{formatMagnitude(preview.net, currency)} net.
			{#if Number(preview.surplus) > 0}
				{formatMagnitude(preview.surplus, currency)} came back beyond what was paid; that part offsets
				nothing.
			{/if}
		{/if}
	</p>

	<section aria-label="Suggestions" class="flex flex-col gap-2">
		<h3 class="text-xs font-semibold tracking-wide text-slate-500 uppercase">
			Likely {anchor.role === 'EXPENSE' ? 'reimbursements' : 'expenses'} for {anchor.label}
		</h3>
		<form
			class="flex gap-2"
			onsubmit={(e) => {
				e.preventDefault();
				loadCandidates();
			}}
		>
			<input
				type="search"
				bind:value={search}
				aria-label="Search all transactions"
				placeholder="Search by name or description"
				class="{input} min-w-0 flex-1"
			/>
			<Button type="submit" variant="secondary">Search</Button>
		</form>
		{#if candidates === null}
			<p class="text-sm text-slate-400">Looking for matches...</p>
		{:else if candidates.length === 0}
			<p class="text-sm text-slate-500">No unlinked transaction fits. Try a search.</p>
		{:else}
			<ul class="flex flex-col divide-y divide-slate-100">
				{#each candidates as c (c.transaction.id)}
					<li class="flex items-center justify-between gap-2 py-1.5 text-sm">
						<span class="min-w-0 truncate">
							<span class="font-medium"
								>{c.transaction.counterpartyName ?? c.transaction.description ?? 'Unnamed'}</span
							>
							<span class="text-slate-500">, {formatDisplayDate(c.transaction.bookingDate)}</span>
						</span>
						<span class="flex shrink-0 items-center gap-2">
							<span class="tabular-nums"
								>{formatMagnitude(c.transaction.amount, c.transaction.currency)}</span
							>
							<Button variant="secondary" onclick={() => add(c)}>Add</Button>
						</span>
					</li>
				{/each}
			</ul>
		{/if}
	</section>

	<label class="flex flex-col gap-1 text-sm font-medium text-slate-700">
		Note
		<input type="text" bind:value={note} maxlength="500" placeholder="Optional" class={input} />
	</label>

	{#if errorMessage}
		<p role="alert" class="text-sm text-[var(--color-spending)]">{errorMessage}</p>
	{:else if blocker}
		<p class="text-xs text-slate-500">{blocker}</p>
	{/if}

	<div class="flex flex-wrap items-center justify-between gap-2">
		{#if link}
			<Button variant="ghost" onclick={unlink} disabled={busy}>Remove link</Button>
		{:else}
			<span></span>
		{/if}
		<div class="flex gap-2">
			<Button variant="ghost" onclick={onclose} disabled={busy}>Cancel</Button>
			<Button variant="primary" onclick={save} disabled={busy || blocker !== null}>
				{link ? 'Save link' : 'Create link'}
			</Button>
		</div>
	</div>
</Modal>
