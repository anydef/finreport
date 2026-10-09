<script lang="ts">
	/**
	 * Create a category without leaving the transaction editor. Calls
	 * `ensureCategory` (create-or-return-existing, so a repeat is not an
	 * error; `createCategory` stays strict for the admin page) and hands the
	 * result to `oncreated`. The slug is composed by `categorySlug.ts` (a
	 * child's slug is its parent's plus exactly one segment), and the full
	 * slug is shown before creating, as the admin dialog does. A server
	 * rejection (`VALIDATION`, naming the expected slug) is shown verbatim.
	 */
	import { untrack } from 'svelte';
	import Button from './Button.svelte';
	import Field from './Field.svelte';
	import { createGraphqlClient } from '$lib/graphqlClient';
	import { ENSURE_CATEGORY_MUTATION } from '$lib/graphql/queries';
	import { SEGMENT_HINT } from '$lib/categorySlug';
	import { defaultKind, leafFromName, parentCandidates, planCategory } from '$lib/categoryCreate';
	import type { Category, CategoryKind } from '$lib/graphql/types';

	interface Props {
		categories: Category[];
		/** Pre-selected parent (e.g. the category being replaced); `null` = top level. */
		parentSlug?: string | null;
		oncreated: (category: Category) => void | Promise<void>;
		oncancel: () => void;
	}

	let { categories, parentSlug: initialParent = null, oncreated, oncancel }: Props = $props();

	const KINDS: CategoryKind[] = ['INCOME', 'EXPENSE', 'TRANSFER', 'SAVING'];
	const uid = $props.id();

	let parentSlug = $state<string | null>(untrack(() => initialParent));
	let name = $state('');
	let leaf = $state('');
	/** Once the user edits the slug by hand it stops following the name. */
	let leafTouched = $state(false);
	let topLevelKind = $state<CategoryKind>('EXPENSE');
	let busy = $state(false);
	let errorMessage = $state('');

	const parents = $derived(parentCandidates(categories));
	const kind = $derived(parentSlug ? defaultKind(parentSlug, categories) : topLevelKind);
	const plan = $derived(planCategory({ parentSlug, leaf, name, kind }));
	/** Only worth complaining about once something has been typed. */
	const showProblem = $derived(!plan.ok && (leaf.trim() !== '' || name.trim() !== ''));

	function onNameInput() {
		if (!leafTouched) leaf = leafFromName(name);
	}

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		if (!plan.ok || busy) return;
		busy = true;
		errorMessage = '';
		try {
			const result = await createGraphqlClient(fetch)
				.mutation(ENSURE_CATEGORY_MUTATION, { input: plan.input })
				.toPromise();
			const created = result.data?.ensureCategory as Category | undefined;
			if (result.error || !created) {
				// urql prefixes "[GraphQL] "; the rest is the server's own message.
				throw new Error(
					result.error?.graphQLErrors[0]?.message ??
						result.error?.message ??
						'The category could not be created.'
				);
			}
			await oncreated(created);
		} catch (err) {
			errorMessage = err instanceof Error ? err.message : 'The category could not be created.';
		} finally {
			busy = false;
		}
	}

	function onKeydown(event: KeyboardEvent) {
		if (event.key !== 'Escape') return;
		event.preventDefault();
		event.stopPropagation();
		oncancel();
	}

	const input = 'rounded-md border-slate-300 text-sm';
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<form
	onsubmit={submit}
	onkeydown={onKeydown}
	aria-label="New category"
	class="flex flex-col gap-3 rounded-md border border-slate-200 bg-white p-3"
>
	<div class="grid gap-3 sm:grid-cols-2">
		<Field label="Parent" for="{uid}-parent">
			<select id="{uid}-parent" bind:value={parentSlug} class={input}>
				<option value={null}>None (top level)</option>
				{#each parents as p (p.slug)}
					<option value={p.slug}>{p.slug} — {p.name}</option>
				{/each}
			</select>
		</Field>
		<Field label="Name" for="{uid}-name">
			<input
				id="{uid}-name"
				bind:value={name}
				oninput={onNameInput}
				required
				autocomplete="off"
				class={input}
			/>
		</Field>
		<Field label="Slug" for="{uid}-slug">
			<input
				id="{uid}-slug"
				bind:value={leaf}
				oninput={() => (leafTouched = true)}
				autocomplete="off"
				aria-describedby="{uid}-slug-hint"
				class="{input} font-mono"
			/>
		</Field>
		{#if !parentSlug}
			<Field label="Kind" for="{uid}-kind">
				<select id="{uid}-kind" bind:value={topLevelKind} class={input}>
					{#each KINDS as k (k)}
						<option value={k}>{k.toLowerCase()}</option>
					{/each}
				</select>
			</Field>
		{/if}
	</div>
	<p id="{uid}-slug-hint" class="text-xs text-slate-500">
		{SEGMENT_HINT}
		{#if plan.ok}
			Will be created as <code class="font-mono text-slate-700" data-testid="slug-preview"
				>{plan.slug}</code
			>
		{:else if showProblem}
			<span class="text-red-600">{plan.problem}</span>
		{/if}
	</p>
	{#if errorMessage}
		<p class="rounded-md bg-red-50 px-3 py-2 text-sm text-red-700" role="alert">{errorMessage}</p>
	{/if}
	<div class="flex justify-end gap-2">
		<Button type="button" variant="ghost" onclick={oncancel}>Cancel</Button>
		<Button type="submit" variant="primary" disabled={!plan.ok || busy}>
			{busy ? 'Creating…' : 'Create and select'}
		</Button>
	</div>
</form>
