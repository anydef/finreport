<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { accountDisplayId, formatAmount, pickCounterparty } from '$lib/transactions';
	import type { PageData } from './$types';

	let { data }: { data: PageData } = $props();

	let startDate = $state(data.startDate);
	let endDate = $state(data.endDate);
	let accountId = $state(data.accountId ?? '');

	// Keep local filter state in sync when navigation updates `data`
	// (e.g. back/forward navigation).
	$effect(() => {
		startDate = data.startDate;
		endDate = data.endDate;
		accountId = data.accountId ?? '';
	});

	function applyFilters() {
		const params = new URLSearchParams(page.url.searchParams);
		if (startDate) {
			params.set('startDate', startDate);
		} else {
			params.delete('startDate');
		}
		if (endDate) {
			params.set('endDate', endDate);
		} else {
			params.delete('endDate');
		}
		if (accountId) {
			params.set('accountId', accountId);
		} else {
			params.delete('accountId');
		}
		goto(`?${params.toString()}`, { keepFocus: true });
	}
</script>

<h1>Transactions</h1>

<form class="filters" onsubmit={(e) => e.preventDefault()}>
	<label>
		Start date
		<input type="date" bind:value={startDate} onchange={applyFilters} />
	</label>
	<label>
		End date
		<input type="date" bind:value={endDate} onchange={applyFilters} />
	</label>
	<label>
		Account
		<select bind:value={accountId} onchange={applyFilters}>
			<option value="">All accounts</option>
			{#each data.accounts as account (account.id)}
				<option value={account.accountId}>{account.displayId}</option>
			{/each}
		</select>
	</label>
</form>

{#if data.error}
	<p role="alert">Failed to load transactions.</p>
{:else if data.transactions.length === 0}
	<p>No transactions in this range.</p>
{:else}
	<table>
		<thead>
			<tr>
				<th>Date</th>
				<th>Description</th>
				<th>Counterparty</th>
				<th>Amount</th>
				<th>Account</th>
			</tr>
		</thead>
		<tbody>
			{#each data.transactions as tx (tx.reference)}
				<tr>
					<td>{tx.bookingDate}</td>
					<td>{tx.remittanceInfo}</td>
					<td>{pickCounterparty(tx)}</td>
					<td class={Number(tx.amount) < 0 ? 'negative' : 'positive'}>
						{formatAmount(tx.amount)}
					</td>
					<td>{accountDisplayId(tx.accountId, data.accounts)}</td>
				</tr>
			{/each}
		</tbody>
	</table>
{/if}

<style>
	.filters {
		display: flex;
		gap: 1rem;
		flex-wrap: wrap;
		margin: 1rem 0;
	}

	.filters label {
		display: flex;
		flex-direction: column;
		font-size: 0.875rem;
		gap: 0.25rem;
	}

	table {
		border-collapse: collapse;
		width: 100%;
	}

	th,
	td {
		text-align: left;
		padding: 0.5rem;
		border-bottom: 1px solid #ddd;
	}

	.negative {
		color: #c0392b;
	}

	.positive {
		color: #27ae60;
	}
</style>
