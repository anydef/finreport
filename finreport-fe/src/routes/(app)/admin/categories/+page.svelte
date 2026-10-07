<script lang="ts">
	/**
	 * `/admin/categories` (§5, §6, §10 WP6): the category tree with
	 * create/rename/archive. There's no `Modal.svelte` here (owned
	 * elsewhere) — the create/rename dialog is a plain native `<dialog>`.
	 */
	import { invalidateAll } from '$app/navigation';
	import Button from '$lib/components/Button.svelte';
	import Card from '$lib/components/Card.svelte';
	import Field from '$lib/components/Field.svelte';
	import CategoryTreeAdmin from '$lib/components/admin/CategoryTreeAdmin.svelte';
	import { createGraphqlClient } from '$lib/graphqlClient';
	import {
		ARCHIVE_CATEGORY_MUTATION,
		CREATE_CATEGORY_MUTATION,
		RENAME_CATEGORY_MUTATION
	} from '$lib/graphql/adminRulesQueries';
	import { buildCategoryTree, type CategoryNode } from '$lib/rulesView';
	import type { CategoryKind } from '$lib/graphql/types';
	import type { PageData } from './$types';

	let { data }: { data: PageData } = $props();

	const tree = $derived(buildCategoryTree(data.categories));

	const KINDS: CategoryKind[] = ['INCOME', 'EXPENSE', 'TRANSFER', 'SAVING'];

	let dialogEl: HTMLDialogElement | undefined = $state();
	let dialogMode = $state<'create' | 'rename' | null>(null);
	let dialogParent = $state<CategoryNode | null>(null);
	let dialogTarget = $state<CategoryNode | null>(null);
	let slugInput = $state('');
	let nameInput = $state('');
	let kindInput = $state<CategoryKind>('EXPENSE');
	let statusMessage = $state<string | null>(null);

	$effect(() => {
		if (!dialogEl) return;
		if (dialogMode && !dialogEl.open) dialogEl.showModal();
		if (!dialogMode && dialogEl.open) dialogEl.close();
	});

	function openCreate(parent: CategoryNode | null) {
		dialogMode = 'create';
		dialogParent = parent;
		dialogTarget = null;
		slugInput = '';
		nameInput = '';
		kindInput = parent?.kind ?? 'EXPENSE';
	}

	function openRename(node: CategoryNode) {
		dialogMode = 'rename';
		dialogTarget = node;
		dialogParent = null;
		nameInput = node.name;
	}

	function closeDialog() {
		dialogMode = null;
		dialogParent = null;
		dialogTarget = null;
	}

	async function archive(node: CategoryNode) {
		const client = createGraphqlClient(fetch);
		await client.mutation(ARCHIVE_CATEGORY_MUTATION, { id: node.id }).toPromise();
		statusMessage = `"${node.name}" archived.`;
		await invalidateAll();
	}

	async function submitDialog(e: SubmitEvent) {
		e.preventDefault();
		const client = createGraphqlClient(fetch);
		if (dialogMode === 'create') {
			await client
				.mutation(CREATE_CATEGORY_MUTATION, {
					input: {
						slug: slugInput.trim(),
						name: nameInput.trim(),
						kind: kindInput,
						parentSlug: dialogParent?.slug ?? null
					}
				})
				.toPromise();
			statusMessage = `"${nameInput}" created.`;
		} else if (dialogMode === 'rename' && dialogTarget) {
			await client
				.mutation(RENAME_CATEGORY_MUTATION, { id: dialogTarget.id, name: nameInput.trim() })
				.toPromise();
			statusMessage = `Renamed to "${nameInput}".`;
		}
		closeDialog();
		await invalidateAll();
	}
</script>

<div class="flex flex-col gap-6">
	<div class="flex items-center justify-between">
		<h1 class="text-lg font-semibold text-slate-900">Categories</h1>
		<Button variant="primary" onclick={() => openCreate(null)}>New top-level category</Button>
	</div>

	{#if data.error}
		<p class="text-sm text-red-600">Failed to load categories.</p>
	{/if}

	{#if statusMessage}
		<p class="rounded-md bg-slate-100 px-3 py-2 text-sm text-slate-700" role="status">
			{statusMessage}
		</p>
	{/if}

	<Card>
		<CategoryTreeAdmin
			nodes={tree}
			oncreatechild={openCreate}
			onrename={openRename}
			onarchive={archive}
		/>
	</Card>
</div>

<dialog
	bind:this={dialogEl}
	onclose={closeDialog}
	class="w-full max-w-md rounded-lg border border-slate-200 p-0 shadow-lg backdrop:bg-slate-900/40"
>
	<form class="flex flex-col gap-4 p-5" onsubmit={submitDialog}>
		<h2 class="text-sm font-semibold text-slate-900">
			{dialogMode === 'create'
				? dialogParent
					? `New child of "${dialogParent.name}"`
					: 'New top-level category'
				: `Rename "${dialogTarget?.name}"`}
		</h2>

		{#if dialogMode === 'create'}
			<Field label="Slug" for="category-slug">
				<input
					id="category-slug"
					required
					pattern={'^[a-z0-9_]+(\\.[a-z0-9_]+){0,2}$'}
					bind:value={slugInput}
					class="rounded-md border-slate-300 text-sm"
				/>
			</Field>
		{/if}

		<Field label="Name" for="category-name">
			<input
				id="category-name"
				required
				bind:value={nameInput}
				class="rounded-md border-slate-300 text-sm"
			/>
		</Field>

		{#if dialogMode === 'create' && !dialogParent}
			<Field label="Kind" for="category-kind">
				<select
					id="category-kind"
					bind:value={kindInput}
					class="rounded-md border-slate-300 text-sm"
				>
					{#each KINDS as kind (kind)}
						<option value={kind}>{kind.toLowerCase()}</option>
					{/each}
				</select>
			</Field>
		{/if}

		<div class="flex justify-end gap-2">
			<Button type="button" variant="ghost" onclick={closeDialog}>Cancel</Button>
			<Button type="submit" variant="primary">Save</Button>
		</div>
	</form>
</dialog>
