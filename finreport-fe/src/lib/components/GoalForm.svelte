<script lang="ts">
	/**
	 * Create/edit form for a goal (iteration 4 §5), rendered inside `Modal`.
	 * Owns only the draft and its validation (`goalsView.ts`); the caller owns
	 * the mutation and closing the modal, like `ReviewCard`'s callbacks.
	 */
	import Button from './Button.svelte';
	import CategoryPicker from './CategoryPicker.svelte';
	import Field from './Field.svelte';
	import TagEditor from './TagEditor.svelte';
	import {
		emptyGoalForm,
		formToInput,
		goalToForm,
		validateGoalForm,
		type GoalFormErrors,
		type GoalFormState
	} from '$lib/goalsView';
	import type { Category, Goal, GoalInput } from '$lib/graphql/types';

	interface Props {
		categories: Category[];
		/** Present when editing; absent when creating. */
		goal?: Goal | null;
		busy?: boolean;
		/** A failure from the last submit, shown above the buttons. */
		error?: string;
		onsubmit: (input: GoalInput) => void;
		oncancel: () => void;
	}

	let { categories, goal = null, busy = false, error = '', onsubmit, oncancel }: Props = $props();

	// The form is mounted fresh per open (inside a conditional `Modal`), so a one-time seed is intended.
	// svelte-ignore state_referenced_locally
	let form = $state<GoalFormState>(goal ? goalToForm(goal) : emptyGoalForm());
	let errors = $state<GoalFormErrors>({});

	const slugNames = $derived(new Map(categories.map((c) => [c.slug, c.name])));
	const inputClass =
		'focus-visible:outline-brand rounded-md border border-slate-300 px-2 py-1.5 text-sm focus-visible:outline focus-visible:outline-2';

	function addCategory(slug: string) {
		if (!form.categorySlugs.includes(slug)) form.categorySlugs = [...form.categorySlugs, slug];
	}

	function removeCategory(slug: string) {
		form.categorySlugs = form.categorySlugs.filter((s) => s !== slug);
	}

	function submit(event: SubmitEvent) {
		event.preventDefault();
		errors = validateGoalForm(form);
		if (Object.keys(errors).length > 0) return;
		onsubmit(formToInput(form, goal?.currency));
	}
</script>

<form onsubmit={submit} class="flex flex-col gap-4" novalidate>
	<Field label="Name" for="goal-name">
		<input id="goal-name" type="text" bind:value={form.name} class={inputClass} />
		{#if errors.name}<span role="alert" class="text-xs text-[var(--color-spending)]"
				>{errors.name}</span
			>{/if}
	</Field>

	<div class="grid grid-cols-2 gap-3">
		<Field label="Type" for="goal-type">
			<select id="goal-type" bind:value={form.type} class={inputClass}>
				<option value="SPENDING_LIMIT">Spending limit</option>
				<option value="SAVING_TARGET">Saving target</option>
			</select>
		</Field>
		<Field label="Amount" for="goal-amount">
			<input
				id="goal-amount"
				type="text"
				inputmode="decimal"
				placeholder="200.00"
				bind:value={form.amount}
				class={inputClass}
			/>
			{#if errors.amount}<span role="alert" class="text-xs text-[var(--color-spending)]"
					>{errors.amount}</span
				>{/if}
		</Field>
	</div>

	<fieldset class="flex flex-col gap-2">
		<legend class="text-sm font-medium text-slate-700">Scope</legend>
		{#if form.categorySlugs.length > 0}
			<ul class="flex flex-wrap gap-1" aria-label="Selected categories">
				{#each form.categorySlugs as slug (slug)}
					<li class="inline-flex items-center gap-1 rounded-full bg-slate-100 px-2 py-0.5 text-xs">
						{slugNames.get(slug) ?? slug}
						<button
							type="button"
							class="text-slate-400 hover:text-slate-700"
							aria-label="Remove category {slugNames.get(slug) ?? slug}"
							onclick={() => removeCategory(slug)}
						>
							×
						</button>
					</li>
				{/each}
			</ul>
		{/if}
		<div class="max-h-48 overflow-y-auto">
			<CategoryPicker
				{categories}
				value={null}
				onchange={addCategory}
				label="Add a category"
				id="goal-categories"
			/>
		</div>
		<div class="flex flex-col gap-1">
			<span class="text-xs font-medium text-slate-500">Tags</span>
			<TagEditor
				tags={form.tags}
				onSave={(tags) => {
					form.tags = tags;
				}}
			/>
		</div>
		{#if form.categorySlugs.length > 0 && form.tags.length > 0}
			<div class="grid grid-cols-2 gap-3">
				<Field label="Categories and tags" for="goal-combine">
					<select id="goal-combine" bind:value={form.combine} class={inputClass}>
						<option value="ALL">Must match both</option>
						<option value="ANY">Either is enough</option>
					</select>
				</Field>
				<Field label="Several tags" for="goal-tag-combine">
					<select id="goal-tag-combine" bind:value={form.tagCombine} class={inputClass}>
						<option value="ALL">All of them</option>
						<option value="ANY">Any of them</option>
					</select>
				</Field>
			</div>
		{:else if form.tags.length > 1}
			<Field label="Several tags" for="goal-tag-combine">
				<select id="goal-tag-combine" bind:value={form.tagCombine} class={inputClass}>
					<option value="ALL">All of them</option>
					<option value="ANY">Any of them</option>
				</select>
			</Field>
		{/if}
		{#if errors.scope}<span role="alert" class="text-xs text-[var(--color-spending)]"
				>{errors.scope}</span
			>{/if}
	</fieldset>

	<div class="grid grid-cols-2 gap-3">
		<Field label="Period" for="goal-period-kind">
			<select id="goal-period-kind" bind:value={form.periodKind} class={inputClass}>
				<option value="RECURRING">Recurring</option>
				<option value="FIXED">Fixed range</option>
			</select>
		</Field>
		{#if form.periodKind === 'RECURRING'}
			<Field label="Every" for="goal-cadence">
				<select id="goal-cadence" bind:value={form.cadence} class={inputClass}>
					<option value="MONTHLY">Month</option>
					<option value="QUARTERLY">Quarter</option>
					<option value="YEARLY">Year</option>
				</select>
			</Field>
		{/if}
	</div>

	{#if form.periodKind === 'FIXED'}
		<div class="grid grid-cols-2 gap-3">
			<Field label="Start" for="goal-start">
				<input id="goal-start" type="date" bind:value={form.startDate} class={inputClass} />
				{#if errors.startDate}<span role="alert" class="text-xs text-[var(--color-spending)]"
						>{errors.startDate}</span
					>{/if}
			</Field>
			<Field label="End (optional)" for="goal-end">
				<input id="goal-end" type="date" bind:value={form.endDate} class={inputClass} />
				{#if errors.endDate}<span role="alert" class="text-xs text-[var(--color-spending)]"
						>{errors.endDate}</span
					>{/if}
			</Field>
		</div>
	{/if}

	{#if error}
		<p role="alert" class="text-sm text-[var(--color-spending)]">{error}</p>
	{/if}

	<div class="flex justify-end gap-2">
		<Button type="button" variant="ghost" onclick={oncancel}>Cancel</Button>
		<Button type="submit" variant="primary" disabled={busy}>
			{goal ? 'Save goal' : 'Add goal'}
		</Button>
	</div>
</form>
