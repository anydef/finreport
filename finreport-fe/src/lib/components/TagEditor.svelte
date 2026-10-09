<script lang="ts">
	/**
	 * Inline tag editor for a transaction row (§5): chips plus a `+` affordance
	 * that opens a text input. Enter adds the typed tag (normalized preview via
	 * `tags.ts`, re-applied server-side); Backspace on an empty input removes
	 * the last tag; Escape cancels the pending input without changing tags.
	 * Calls `onSave` with the *full* resulting tag set, matching
	 * `setTransactionTags`'s whole-state-replace semantics (§4).
	 *
	 * Typing suggests existing tags from `Query.tags` (the whole book, with
	 * usage counts) in normalised form; ArrowUp/Down highlight one, Enter
	 * accepts it, and with nothing highlighted Enter adds the typed text, so a
	 * brand-new tag always works. The list is fetched when editing starts and
	 * is best-effort: if it fails the editor simply behaves as it did before.
	 */
	import TagChips from './TagChips.svelte';
	import { addTag, isValidTag, removeTag } from '$lib/tags';
	import { moveHighlight, suggestTags } from '$lib/tagSuggestions';
	import { createGraphqlClient } from '$lib/graphqlClient';
	import { TAGS_QUERY } from '$lib/graphql/queries';
	import type { TagCount } from '$lib/graphql/types';

	interface Props {
		tags: string[];
		onSave: (tags: string[]) => void | Promise<void>;
	}

	let { tags, onSave }: Props = $props();

	let editing = $state(false);
	let draft = $state('');
	let inputEl = $state<HTMLInputElement | null>(null);
	let known = $state<TagCount[]>([]);
	/** Highlighted suggestion; -1 = none, so Enter adds the typed text. */
	let active = $state(-1);

	const uid = $props.id();
	const listId = `${uid}-tag-suggestions`;
	const suggestions = $derived(editing ? suggestTags(known, draft, tags) : []);

	async function loadKnown() {
		try {
			const result = await createGraphqlClient(fetch).query(TAGS_QUERY, {}).toPromise();
			known = (result.data?.tags ?? []) as TagCount[];
		} catch {
			known = [];
		}
	}

	function startEditing() {
		editing = true;
		draft = '';
		active = -1;
		loadKnown();
		queueMicrotask(() => inputEl?.focus());
	}

	function commit(raw: string) {
		draft = '';
		active = -1;
		if (!isValidTag(raw)) return;
		onSave(addTag(tags, raw));
	}

	function removeTagAt(tag: string) {
		onSave(removeTag(tags, tag));
	}

	function onKeydown(event: KeyboardEvent) {
		if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
			event.preventDefault();
			active = moveHighlight(active, event.key, suggestions.length);
		} else if (event.key === 'Enter') {
			event.preventDefault();
			commit(suggestions[active]?.tag ?? draft);
		} else if (event.key === 'Backspace' && draft === '' && tags.length > 0) {
			event.preventDefault();
			removeTagAt(tags[tags.length - 1]);
		} else if (event.key === 'Escape') {
			// Consumed here: the first Esc cancels the input, the next one
			// reaches the row editor and collapses it.
			event.preventDefault();
			event.stopPropagation();
			editing = false;
			draft = '';
		}
	}

	function onBlur() {
		if (draft === '') editing = false;
	}
</script>

<div class="relative flex flex-wrap items-center gap-1">
	<TagChips {tags} onRemove={removeTagAt} />
	{#if editing}
		<input
			bind:this={inputEl}
			bind:value={draft}
			oninput={() => (active = -1)}
			onkeydown={onKeydown}
			onblur={onBlur}
			type="text"
			role="combobox"
			aria-label="Add tag"
			aria-expanded={suggestions.length > 0}
			aria-controls={listId}
			aria-autocomplete="list"
			aria-activedescendant={active >= 0 ? `${uid}-tag-${active}` : undefined}
			autocomplete="off"
			placeholder="tag"
			class="w-32 rounded-md border border-slate-300 px-1 py-0.5 text-xs"
		/>
		{#if suggestions.length > 0}
			<!-- mousedown is cancelled so the input keeps focus (and its blur
			     handler does not close the editor) while a suggestion is clicked. -->
			<ul
				id={listId}
				role="listbox"
				aria-label="Tag suggestions"
				class="absolute top-full left-0 z-10 mt-1 max-h-56 w-60 overflow-y-auto rounded-md border border-slate-200 bg-white py-1 text-xs shadow-lg"
			>
				{#each suggestions as s, i (s.tag)}
					<li
						id="{uid}-tag-{i}"
						role="option"
						aria-selected={i === active}
						class="flex cursor-pointer items-center justify-between gap-2 px-3 py-1.5 text-slate-800 {i ===
						active
							? 'bg-slate-100'
							: ''}"
						onmousedown={(e) => e.preventDefault()}
						onmouseenter={() => (active = i)}
						onclick={() => commit(s.tag)}
						onkeydown={() => {}}
					>
						<span>{s.isNew ? `Add “${s.tag}”` : s.tag}</span>
						<span class="text-slate-400">
							{s.isNew ? 'new tag' : `${s.count} ${s.count === 1 ? 'transaction' : 'transactions'}`}
						</span>
					</li>
				{/each}
			</ul>
		{/if}
	{:else}
		<button
			type="button"
			class="inline-flex h-5 w-5 items-center justify-center rounded-full text-xs text-slate-400 hover:bg-slate-100 hover:text-slate-700"
			aria-label="Add tag"
			onclick={startEditing}
		>
			+
		</button>
	{/if}
</div>
