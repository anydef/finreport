<script lang="ts">
	/**
	 * Add / edit one cash transaction. The caller passes the starting values:
	 * for an edit they are every field of the existing row, because
	 * `updateCashTransaction` replaces the whole record. Validation is the pure
	 * `validateCashForm`; the server's own refusal arrives as `serverError`.
	 */
	import Button from '$lib/components/Button.svelte';
	import Field from '$lib/components/Field.svelte';
	import { COUNTERPARTY_MAX, DESCRIPTION_MAX, validateCashForm } from '$lib/cashView';
	import type { CashFormErrors, CashFormValues } from '$lib/cashView';
	import type { CashTransactionInput } from '$lib/graphql/types';

	interface Props {
		initial: CashFormValues;
		accountCurrency: string;
		submitLabel: string;
		busy: boolean;
		serverError: string | null;
		onsubmit: (input: CashTransactionInput) => void;
		oncancel: () => void;
	}

	let { initial, accountCurrency, submitLabel, busy, serverError, onsubmit, oncancel }: Props =
		$props();

	// Seeded once per mount: the page remounts the form for each add/edit.
	let values = $state<CashFormValues>({ ...initial });
	let errors = $state<CashFormErrors>({});

	const inputClass =
		'focus-visible:outline-brand rounded-md border border-slate-300 px-2 py-1.5 text-sm focus-visible:outline focus-visible:outline-2';

	function submit(event: SubmitEvent) {
		event.preventDefault();
		const result = validateCashForm(values, accountCurrency);
		errors = result.errors;
		if (result.input) onsubmit(result.input);
	}
</script>

<form class="flex flex-col gap-3" onsubmit={submit} novalidate>
	<div class="grid grid-cols-2 gap-3">
		<Field label="Date" for="cash-date">
			<input id="cash-date" type="date" bind:value={values.bookingDate} class={inputClass} />
			{#if errors.bookingDate}<span role="alert" class="text-xs text-red-600"
					>{errors.bookingDate}</span
				>{/if}
		</Field>
		<Field label="Type" for="cash-direction">
			<select id="cash-direction" bind:value={values.direction} class={inputClass}>
				<option value="spent">Spent</option>
				<option value="received">Received</option>
			</select>
		</Field>
	</div>

	<div class="grid grid-cols-2 gap-3">
		<Field label="Amount" for="cash-amount">
			<input
				id="cash-amount"
				type="text"
				inputmode="decimal"
				placeholder="12.50"
				bind:value={values.amount}
				class={inputClass}
			/>
			{#if errors.amount}<span role="alert" class="text-xs text-red-600">{errors.amount}</span>{/if}
		</Field>
		<Field label="Currency" for="cash-currency">
			<input
				id="cash-currency"
				type="text"
				maxlength="3"
				bind:value={values.currency}
				class="{inputClass} uppercase"
			/>
			{#if errors.currency}<span role="alert" class="text-xs text-red-600">{errors.currency}</span
				>{/if}
		</Field>
	</div>

	<Field label="Who (optional)" for="cash-counterparty">
		<input
			id="cash-counterparty"
			type="text"
			maxlength={COUNTERPARTY_MAX}
			bind:value={values.counterpartyName}
			class={inputClass}
		/>
		{#if errors.counterpartyName}<span role="alert" class="text-xs text-red-600"
				>{errors.counterpartyName}</span
			>{/if}
	</Field>

	<Field label="What for (optional)" for="cash-description">
		<input
			id="cash-description"
			type="text"
			maxlength={DESCRIPTION_MAX}
			bind:value={values.description}
			class={inputClass}
		/>
		{#if errors.description}<span role="alert" class="text-xs text-red-600"
				>{errors.description}</span
			>{/if}
	</Field>

	{#if serverError}
		<p class="text-sm text-red-600" role="alert">{serverError}</p>
	{/if}

	<div class="flex justify-end gap-2">
		<Button type="button" variant="ghost" onclick={oncancel} disabled={busy}>Cancel</Button>
		<Button type="submit" variant="primary" disabled={busy}>
			{busy ? 'Saving…' : submitLabel}
		</Button>
	</div>
</form>
