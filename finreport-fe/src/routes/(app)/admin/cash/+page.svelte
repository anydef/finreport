<script lang="ts">
	/**
	 * `/admin/cash`: hand-kept cash accounts ("ephemeral": the user may delete
	 * an account and all its transactions at will). Not admin-only: it is the
	 * caller's own data. A cash account has no bank balance, so the balance
	 * shown is the running sum of its own rows (`cashView.ts`).
	 *
	 * Everything is refetched after a write so the page shows what the server
	 * holds; the server's refusal text is rendered, not assumed.
	 */
	import { untrack } from 'svelte';
	import Button from '$lib/components/Button.svelte';
	import Card from '$lib/components/Card.svelte';
	import CashTransactionForm from '$lib/components/CashTransactionForm.svelte';
	import Field from '$lib/components/Field.svelte';
	import Modal from '$lib/components/Modal.svelte';
	import {
		balanceTone,
		cashAccounts,
		cashEmptyState,
		cashErrorMessage,
		cashTotals,
		deleteAccountWarning,
		deleteTransactionWarning,
		emptyCashForm,
		formFromTransaction,
		LABEL_MAX,
		suggestedCurrency,
		validateAccountForm,
		withRunningBalance
	} from '$lib/cashView';
	import type { CashAction } from '$lib/cashView';
	import { formatAmount, formatDisplayDate } from '$lib/format';
	import { createGraphqlClient } from '$lib/graphqlClient';
	import {
		CREATE_CASH_ACCOUNT_MUTATION,
		CREATE_CASH_TRANSACTION_MUTATION,
		DELETE_CASH_ACCOUNT_MUTATION,
		DELETE_CASH_TRANSACTION_MUTATION,
		RENAME_CASH_ACCOUNT_MUTATION,
		UPDATE_CASH_TRANSACTION_MUTATION
	} from '$lib/graphql/adminCashQueries';
	import { ACCOUNTS_QUERY, TRANSACTIONS_QUERY } from '$lib/graphql/queries';
	import type { Account, CashTransactionInput, Transaction } from '$lib/graphql/types';
	import { toDateInputValue } from '$lib/period';
	import type { PageData } from './$types';

	let { data }: { data: PageData } = $props();

	type Dialog =
		| { kind: 'create-account' }
		| { kind: 'rename-account' }
		| { kind: 'delete-account' }
		| { kind: 'add' }
		| { kind: 'edit'; transaction: Transaction }
		| { kind: 'delete-transaction'; transaction: Transaction };

	const PAGE_SIZE = 200;
	const inputClass =
		'focus-visible:outline-brand rounded-md border border-slate-300 px-2 py-1.5 text-sm focus-visible:outline focus-visible:outline-2';

	let allAccounts = $state<Account[]>(untrack(() => data.accounts));
	let selectedId = $state<string | null>(null);
	let rows = $state<Transaction[]>([]);
	let rowsLoading = $state(false);
	let pageError = $state<string | null>(null);
	let loadFailed = $state(untrack(() => data.error));
	let dialog = $state<Dialog | null>(null);
	let dialogError = $state<string | null>(null);
	let busy = $state(false);
	let statusMessage = $state<string | null>(null);
	let accountLabel = $state('');
	let accountCurrency = $state('');
	let rowsRequest = 0;

	const cash = $derived(cashAccounts(allAccounts));
	const selected = $derived(cash.find((a) => a.id === selectedId) ?? cash[0] ?? null);
	const ledger = $derived(withRunningBalance(rows));
	const totals = $derived(cashTotals(rows));
	const empty = $derived(cashEmptyState(cash.length, rows.length));
	const balanceClass = {
		positive: 'text-[var(--color-income)]',
		negative: 'text-[var(--color-spending)]',
		zero: 'text-slate-700'
	};

	function errorText(
		result: {
			error?: { graphQLErrors: { message: string; extensions?: Record<string, unknown> }[] };
		},
		action: CashAction
	): string {
		const first = result.error?.graphQLErrors[0];
		return cashErrorMessage(first?.extensions?.code as string | undefined, first?.message, action);
	}

	async function loadAccounts(): Promise<boolean> {
		const result = await createGraphqlClient(fetch).query(ACCOUNTS_QUERY, {}).toPromise();
		if (result.error) {
			pageError = errorText(result, 'load');
			return false;
		}
		allAccounts = (result.data?.accounts ?? []) as Account[];
		return true;
	}

	async function loadRows(accountId: string) {
		const ticket = ++rowsRequest;
		rowsLoading = true;
		const collected: Transaction[] = [];
		const client = createGraphqlClient(fetch);
		for (let offset = 0; ; offset += PAGE_SIZE) {
			const result = await client
				.query(TRANSACTIONS_QUERY, {
					filter: { accountIds: [accountId] },
					page: { limit: PAGE_SIZE, offset }
				})
				.toPromise();
			if (ticket !== rowsRequest) return;
			if (result.error || !result.data) {
				pageError = errorText(result, 'load');
				break;
			}
			const page = result.data.transactions as { items: Transaction[]; totalCount: number };
			collected.push(...page.items);
			if (collected.length >= page.totalCount || page.items.length === 0) break;
		}
		rows = collected;
		rowsLoading = false;
	}

	// Load the rows of whichever account is showing.
	$effect(() => {
		const id = selected?.id;
		if (id) untrack(() => loadRows(id));
		else rows = [];
	});

	function open(next: Dialog) {
		dialogError = null;
		statusMessage = null;
		if (next.kind === 'create-account') {
			accountLabel = '';
			accountCurrency = suggestedCurrency(allAccounts) ?? '';
		}
		if (next.kind === 'rename-account')
			accountLabel = selected?.label ?? selected?.displayName ?? '';
		dialog = next;
	}

	function close() {
		if (!busy) dialog = null;
	}

	/** Run one mutation: show its refusal inside the dialog, or close and refresh. */
	async function commit(
		action: CashAction,
		run: () => Promise<{
			error?: { graphQLErrors: { message: string; extensions?: Record<string, unknown> }[] };
		}>,
		done: string
	): Promise<boolean> {
		busy = true;
		dialogError = null;
		pageError = null;
		const result = await run();
		if (result.error) {
			dialogError = errorText(result, action);
			busy = false;
			return false;
		}
		dialog = null;
		statusMessage = done;
		await loadAccounts();
		if (selected) await loadRows(selected.id);
		busy = false;
		return true;
	}

	async function createAccount(event: SubmitEvent) {
		event.preventDefault();
		const v = validateAccountForm(accountLabel, accountCurrency);
		if (v.errors.label || v.errors.currency) {
			dialogError = v.errors.label ?? v.errors.currency ?? null;
			return;
		}
		let createdId: string | null = null;
		const ok = await commit(
			'create-account',
			async () => {
				const result = await createGraphqlClient(fetch)
					.mutation(CREATE_CASH_ACCOUNT_MUTATION, { label: v.label, currency: v.currency })
					.toPromise();
				createdId = (result.data?.createCashAccount as { id: string } | undefined)?.id ?? null;
				return result;
			},
			`Created "${v.label}".`
		);
		if (ok && createdId) selectedId = createdId;
	}

	async function renameAccount(event: SubmitEvent) {
		event.preventDefault();
		if (!selected) return;
		const v = validateAccountForm(accountLabel, null);
		if (v.errors.label) {
			dialogError = v.errors.label;
			return;
		}
		const accountId = selected.id;
		await commit(
			'rename-account',
			() =>
				createGraphqlClient(fetch)
					.mutation(RENAME_CASH_ACCOUNT_MUTATION, { accountId, label: v.label })
					.toPromise(),
			`Renamed to "${v.label}".`
		);
	}

	async function deleteAccount() {
		if (!selected) return;
		const accountId = selected.id;
		const label = selected.displayName;
		const ok = await commit(
			'delete-account',
			() =>
				createGraphqlClient(fetch)
					.mutation(DELETE_CASH_ACCOUNT_MUTATION, { accountId })
					.toPromise(),
			`Deleted "${label}" and its transactions.`
		);
		if (ok) selectedId = null;
	}

	async function saveTransaction(input: CashTransactionInput) {
		if (!selected || !dialog) return;
		const accountId = selected.id;
		const current = dialog;
		await commit(
			'save-transaction',
			() =>
				current.kind === 'edit'
					? createGraphqlClient(fetch)
							.mutation(UPDATE_CASH_TRANSACTION_MUTATION, {
								transactionId: current.transaction.id,
								input
							})
							.toPromise()
					: createGraphqlClient(fetch)
							.mutation(CREATE_CASH_TRANSACTION_MUTATION, { accountId, input })
							.toPromise(),
			current.kind === 'edit' ? 'Transaction updated.' : 'Transaction added.'
		);
	}

	async function deleteTransaction(transaction: Transaction) {
		await commit(
			'delete-transaction',
			() =>
				createGraphqlClient(fetch)
					.mutation(DELETE_CASH_TRANSACTION_MUTATION, { transactionId: transaction.id })
					.toPromise(),
			'Transaction deleted.'
		);
	}

	const today = () => toDateInputValue(new Date());
</script>

<div class="flex flex-col gap-6">
	<div class="flex flex-wrap items-center justify-between gap-2">
		<h1 class="text-lg font-semibold text-slate-900">Cash accounts</h1>
		<Button variant="primary" onclick={() => open({ kind: 'create-account' })}>
			New cash account
		</Button>
	</div>

	{#if loadFailed && cash.length === 0}
		<p class="text-sm text-red-600" role="alert">Failed to load accounts.</p>
	{/if}
	{#if pageError}
		<p class="text-sm text-red-600" role="alert">{pageError}</p>
	{/if}
	{#if statusMessage}
		<p class="rounded-md bg-slate-100 px-3 py-2 text-sm text-slate-700" role="status">
			{statusMessage}
		</p>
	{/if}

	{#if empty === 'no-accounts'}
		<Card>
			<p class="text-sm text-slate-600">
				No cash accounts yet. Create one to track money you spend in cash: a wallet, a cash tin, a
				holiday float. Deleting an account later removes it and all its transactions.
			</p>
		</Card>
	{:else if selected}
		<Card>
			<div class="flex flex-col gap-3 sm:flex-row sm:items-end sm:justify-between">
				<div class="flex flex-col gap-1">
					{#if cash.length > 1}
						<label for="cash-account" class="text-xs font-medium text-slate-500">Account</label>
						<select
							id="cash-account"
							class={inputClass}
							value={selected.id}
							onchange={(e) => (selectedId = e.currentTarget.value)}
						>
							{#each cash as account (account.id)}
								<option value={account.id}>{account.displayName} ({account.currency})</option>
							{/each}
						</select>
					{:else}
						<p class="text-base font-semibold text-slate-900">
							{selected.displayName}
							<span class="text-sm font-normal text-slate-500">({selected.currency})</span>
						</p>
					{/if}
				</div>
				<div class="flex gap-2">
					<Button onclick={() => open({ kind: 'rename-account' })}>Rename</Button>
					<Button onclick={() => open({ kind: 'delete-account' })}>Delete account</Button>
				</div>
			</div>
			<dl class="mt-4 grid grid-cols-3 gap-3 text-sm">
				<div>
					<dt class="text-xs text-slate-500">Balance</dt>
					<dd class="font-semibold {balanceClass[balanceTone(totals.balance)]}">
						{formatAmount(totals.balance, selected.currency)}
					</dd>
				</div>
				<div>
					<dt class="text-xs text-slate-500">Received</dt>
					<dd class="text-slate-800">{formatAmount(totals.received, selected.currency)}</dd>
				</div>
				<div>
					<dt class="text-xs text-slate-500">Spent</dt>
					<dd class="text-slate-800">{formatAmount(`-${totals.spent}`, selected.currency)}</dd>
				</div>
			</dl>
		</Card>

		<Card title="Transactions">
			<div class="mb-3 flex items-center justify-between gap-2">
				<p class="text-sm text-slate-600">
					The balance is the running sum of these rows; a cash account has no bank balance.
				</p>
				<Button variant="primary" onclick={() => open({ kind: 'add' })}>Add transaction</Button>
			</div>
			{#if rowsLoading && rows.length === 0}
				<p class="text-sm text-slate-500">Loading…</p>
			{:else if empty === 'no-transactions'}
				<p class="text-sm text-slate-600">No transactions yet. Add the first one.</p>
			{:else}
				<ul class="divide-y divide-slate-100">
					{#each ledger as row (row.transaction.id)}
						<li class="flex flex-col gap-2 py-2 sm:flex-row sm:items-center sm:gap-4">
							<div class="min-w-0 flex-1">
								<p class="truncate text-sm font-medium text-slate-900">
									{row.transaction.counterpartyName ?? row.transaction.description ?? 'Cash'}
								</p>
								<p class="truncate text-xs text-slate-500">
									{formatDisplayDate(row.transaction.bookingDate)}{row.transaction
										.counterpartyName && row.transaction.description
										? ` · ${row.transaction.description}`
										: ''}
								</p>
								{#if row.transferNote}
									<!--
										The transfer detector pairs a cash row with the bank leg of
										the same movement (an ATM withdrawal, say) on its next pass.
										Shown because the pairing is what stops the money being
										counted as both bank spending and cash income — and because
										it happens asynchronously, so without this the user cannot
										tell a linked row from one still waiting.
									-->
									<p
										class="mt-1 inline-flex items-center rounded bg-slate-100 px-1.5 py-0.5 text-xs text-slate-600"
									>
										{row.transferNote}
									</p>
								{/if}
							</div>
							<div class="flex items-baseline justify-between gap-4 sm:justify-end">
								<span class="text-sm font-medium text-slate-900">
									{formatAmount(row.transaction.amount, row.transaction.currency)}
								</span>
								<span class="text-xs text-slate-500" title="Balance after this row">
									Bal. {formatAmount(row.balance, row.transaction.currency)}
								</span>
							</div>
							<div class="flex gap-1">
								<Button
									variant="ghost"
									onclick={() => open({ kind: 'edit', transaction: row.transaction })}
									aria-label="Edit transaction"
								>
									Edit
								</Button>
								<Button
									variant="ghost"
									onclick={() => open({ kind: 'delete-transaction', transaction: row.transaction })}
									aria-label="Delete transaction"
								>
									Delete
								</Button>
							</div>
						</li>
					{/each}
				</ul>
			{/if}
		</Card>
	{/if}
</div>

{#if dialog?.kind === 'create-account'}
	<Modal title="New cash account" onclose={close}>
		<form class="flex flex-col gap-3" onsubmit={createAccount} novalidate>
			<Field label="Name" for="cash-new-label">
				<input
					id="cash-new-label"
					type="text"
					maxlength={LABEL_MAX}
					placeholder="Wallet"
					bind:value={accountLabel}
					class={inputClass}
				/>
			</Field>
			<Field label="Currency" for="cash-new-currency">
				<input
					id="cash-new-currency"
					type="text"
					maxlength="3"
					placeholder="EUR"
					bind:value={accountCurrency}
					class="{inputClass} uppercase"
				/>
			</Field>
			<p class="text-xs text-slate-500">
				The currency must match your other accounts, so totals are never mixed.
			</p>
			{#if dialogError}<p class="text-sm text-red-600" role="alert">{dialogError}</p>{/if}
			<div class="flex justify-end gap-2">
				<Button type="button" variant="ghost" onclick={close} disabled={busy}>Cancel</Button>
				<Button type="submit" variant="primary" disabled={busy}>
					{busy ? 'Creating…' : 'Create'}
				</Button>
			</div>
		</form>
	</Modal>
{:else if dialog?.kind === 'rename-account'}
	<Modal title="Rename cash account" onclose={close}>
		<form class="flex flex-col gap-3" onsubmit={renameAccount} novalidate>
			<Field label="Name" for="cash-rename-label">
				<input
					id="cash-rename-label"
					type="text"
					maxlength={LABEL_MAX}
					bind:value={accountLabel}
					class={inputClass}
				/>
			</Field>
			{#if dialogError}<p class="text-sm text-red-600" role="alert">{dialogError}</p>{/if}
			<div class="flex justify-end gap-2">
				<Button type="button" variant="ghost" onclick={close} disabled={busy}>Cancel</Button>
				<Button type="submit" variant="primary" disabled={busy}>
					{busy ? 'Saving…' : 'Rename'}
				</Button>
			</div>
		</form>
	</Modal>
{:else if dialog?.kind === 'delete-account' && selected}
	<Modal title="Delete cash account" onclose={close}>
		<p class="text-sm text-slate-700">{deleteAccountWarning(selected.displayName, rows.length)}</p>
		{#if dialogError}<p class="text-sm text-red-600" role="alert">{dialogError}</p>{/if}
		<div class="flex justify-end gap-2">
			<Button variant="ghost" onclick={close} disabled={busy}>Keep it</Button>
			<Button
				variant="primary"
				class="bg-red-600 hover:bg-red-700"
				onclick={deleteAccount}
				disabled={busy}
			>
				{busy ? 'Deleting…' : `Delete "${selected.displayName}" permanently`}
			</Button>
		</div>
	</Modal>
{:else if dialog?.kind === 'add' && selected}
	<Modal title="Add transaction" onclose={close}>
		<CashTransactionForm
			initial={emptyCashForm(today(), selected.currency)}
			accountCurrency={selected.currency}
			submitLabel="Add"
			{busy}
			serverError={dialogError}
			onsubmit={saveTransaction}
			oncancel={close}
		/>
	</Modal>
{:else if dialog?.kind === 'edit' && selected}
	<Modal title="Edit transaction" onclose={close}>
		<CashTransactionForm
			initial={formFromTransaction(dialog.transaction)}
			accountCurrency={selected.currency}
			submitLabel="Save"
			{busy}
			serverError={dialogError}
			onsubmit={saveTransaction}
			oncancel={close}
		/>
	</Modal>
{:else if dialog?.kind === 'delete-transaction'}
	{@const target = dialog.transaction}
	<Modal title="Delete transaction" onclose={close}>
		<p class="text-sm text-slate-700">{deleteTransactionWarning()}</p>
		<p class="text-sm font-medium text-slate-900">
			{formatDisplayDate(target.bookingDate)} · {formatAmount(target.amount, target.currency)}
		</p>
		{#if dialogError}<p class="text-sm text-red-600" role="alert">{dialogError}</p>{/if}
		<div class="flex justify-end gap-2">
			<Button variant="ghost" onclick={close} disabled={busy}>Keep it</Button>
			<Button
				variant="primary"
				class="bg-red-600 hover:bg-red-700"
				onclick={() => deleteTransaction(target)}
				disabled={busy}
			>
				{busy ? 'Deleting…' : 'Delete permanently'}
			</Button>
		</div>
	</Modal>
{/if}
