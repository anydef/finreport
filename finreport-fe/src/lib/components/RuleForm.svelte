<script lang="ts">
	/**
	 * Create/edit form for a user rule (§2.7, §6 "inline create/edit"),
	 * rendered inside a native `<dialog>` — this repo has no `Modal.svelte`
	 * (owned elsewhere), so each component that needs a modal brings its own
	 * `<dialog>`.
	 */
	import Button from './Button.svelte';
	import Field from './Field.svelte';
	import { flattenCategoryTreeWithPath, buildCategoryTree } from '$lib/rulesView';
	import type { Category, Rule, RuleInput } from '$lib/graphql/types';
	import type { RuleConditions } from '$lib/rulesView';

	interface Props {
		open: boolean;
		categories: Category[];
		/** Present when editing; absent when creating a new rule. */
		rule?: Rule | null;
		onsubmit: (input: RuleInput) => void;
		onclose: () => void;
	}

	let { open, categories, rule = null, onsubmit, onclose }: Props = $props();

	let dialogEl: HTMLDialogElement | undefined = $state();

	const categoryOptions = $derived(
		flattenCategoryTreeWithPath(buildCategoryTree(categories.filter((c) => !c.archived)))
	);

	function conditionsOf(r: Rule | null): RuleConditions {
		return (r?.conditions ?? {}) as RuleConditions;
	}

	let name = $state('');
	let categorySlug = $state('');
	let priority = $state(0);
	let counterpartyKey = $state('');
	let counterpartyIban = $state('');
	let descriptionContains = $state('');
	let descriptionRegex = $state('');
	let direction = $state<'' | 'INCOME' | 'SPENDING'>('');
	let amountMin = $state('');
	let amountMax = $state('');

	/** Re-seed every field whenever the dialog opens or the rule being edited changes. */
	$effect(() => {
		if (!open) return;
		const c = conditionsOf(rule);
		name = rule?.name ?? '';
		categorySlug = rule?.category.slug ?? categoryOptions[0]?.category.slug ?? '';
		priority = rule?.priority ?? 0;
		counterpartyKey = c.counterpartyKey ?? '';
		counterpartyIban = c.counterpartyIban ?? '';
		descriptionContains = c.descriptionContains ?? '';
		descriptionRegex = c.descriptionRegex ?? '';
		direction = c.direction ?? '';
		amountMin = c.amountMin ?? '';
		amountMax = c.amountMax ?? '';
	});

	$effect(() => {
		if (!dialogEl) return;
		if (open && !dialogEl.open) dialogEl.showModal();
		if (!open && dialogEl.open) dialogEl.close();
	});

	function buildConditions(): Record<string, unknown> {
		const conditions: Record<string, unknown> = {};
		if (counterpartyKey.trim()) conditions.counterpartyKey = counterpartyKey.trim();
		if (counterpartyIban.trim()) conditions.counterpartyIban = counterpartyIban.trim();
		if (descriptionContains.trim()) conditions.descriptionContains = descriptionContains.trim();
		if (descriptionRegex.trim()) conditions.descriptionRegex = descriptionRegex.trim();
		if (direction) conditions.direction = direction;
		if (amountMin.trim()) conditions.amountMin = amountMin.trim();
		if (amountMax.trim()) conditions.amountMax = amountMax.trim();
		return conditions;
	}

	function handleSubmit(e: SubmitEvent) {
		e.preventDefault();
		onsubmit({
			name: name.trim(),
			categorySlug,
			conditions: buildConditions(),
			priority
		});
	}
</script>

<dialog
	bind:this={dialogEl}
	{onclose}
	class="w-full max-w-lg rounded-lg border border-slate-200 p-0 shadow-lg backdrop:bg-slate-900/40"
>
	<form class="flex flex-col gap-4 p-5" onsubmit={handleSubmit}>
		<h2 class="text-sm font-semibold text-slate-900">
			{rule ? 'Edit rule' : 'Create rule'}
		</h2>

		<Field label="Name" for="rule-name">
			<input
				id="rule-name"
				required
				bind:value={name}
				class="rounded-md border-slate-300 text-sm"
			/>
		</Field>

		<Field label="Target category" for="rule-category">
			<select
				id="rule-category"
				required
				bind:value={categorySlug}
				class="rounded-md border-slate-300 text-sm"
			>
				{#each categoryOptions as option (option.category.id)}
					<option value={option.category.slug}>{option.path}</option>
				{/each}
			</select>
		</Field>

		<Field label="Priority" for="rule-priority">
			<input
				id="rule-priority"
				type="number"
				bind:value={priority}
				class="rounded-md border-slate-300 text-sm"
			/>
		</Field>

		<fieldset class="flex flex-col gap-3 rounded-md border border-slate-200 p-3">
			<legend class="px-1 text-xs font-medium text-slate-500 uppercase">
				Match conditions (all present ones AND together)
			</legend>
			<Field label="Counterparty (normalized, exact)" for="rule-counterparty-key">
				<input
					id="rule-counterparty-key"
					bind:value={counterpartyKey}
					class="rounded-md border-slate-300 text-sm"
				/>
			</Field>
			<Field label="Counterparty IBAN" for="rule-counterparty-iban">
				<input
					id="rule-counterparty-iban"
					bind:value={counterpartyIban}
					class="rounded-md border-slate-300 text-sm"
				/>
			</Field>
			<Field label="Description contains" for="rule-description-contains">
				<input
					id="rule-description-contains"
					bind:value={descriptionContains}
					class="rounded-md border-slate-300 text-sm"
				/>
			</Field>
			<Field label="Description matches regex" for="rule-description-regex">
				<input
					id="rule-description-regex"
					bind:value={descriptionRegex}
					class="rounded-md border-slate-300 text-sm"
				/>
			</Field>
			<Field label="Direction" for="rule-direction">
				<select
					id="rule-direction"
					bind:value={direction}
					class="rounded-md border-slate-300 text-sm"
				>
					<option value="">Either</option>
					<option value="INCOME">Income</option>
					<option value="SPENDING">Spending</option>
				</select>
			</Field>
			<div class="flex gap-3">
				<Field label="Amount min" for="rule-amount-min" class="flex-1">
					<input
						id="rule-amount-min"
						bind:value={amountMin}
						class="w-full rounded-md border-slate-300 text-sm"
					/>
				</Field>
				<Field label="Amount max" for="rule-amount-max" class="flex-1">
					<input
						id="rule-amount-max"
						bind:value={amountMax}
						class="w-full rounded-md border-slate-300 text-sm"
					/>
				</Field>
			</div>
		</fieldset>

		<div class="flex justify-end gap-2">
			<Button type="button" variant="ghost" onclick={onclose}>Cancel</Button>
			<Button type="submit" variant="primary">{rule ? 'Save' : 'Create'}</Button>
		</div>
	</form>
</dialog>
