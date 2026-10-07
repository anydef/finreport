<script lang="ts">
	/**
	 * Inline tag editor for a transaction row (§5): chips plus a `+` affordance
	 * that opens a text input. Enter adds the typed tag (normalized preview via
	 * `tags.ts`, re-applied server-side); Backspace on an empty input removes
	 * the last tag; Escape cancels the pending input without changing tags.
	 * Calls `onSave` with the *full* resulting tag set, matching
	 * `setTransactionTags`'s whole-state-replace semantics (§4).
	 */
	import TagChips from './TagChips.svelte';
	import { addTag, isValidTag, removeTag } from '$lib/tags';

	interface Props {
		tags: string[];
		onSave: (tags: string[]) => void | Promise<void>;
	}

	let { tags, onSave }: Props = $props();

	let editing = $state(false);
	let draft = $state('');
	let inputEl = $state<HTMLInputElement | null>(null);

	function startEditing() {
		editing = true;
		draft = '';
		queueMicrotask(() => inputEl?.focus());
	}

	function commitDraft() {
		if (!isValidTag(draft)) {
			draft = '';
			return;
		}
		const next = addTag(tags, draft);
		draft = '';
		onSave(next);
	}

	function removeTagAt(tag: string) {
		onSave(removeTag(tags, tag));
	}

	function onKeydown(event: KeyboardEvent) {
		if (event.key === 'Enter') {
			event.preventDefault();
			commitDraft();
		} else if (event.key === 'Backspace' && draft === '' && tags.length > 0) {
			event.preventDefault();
			removeTagAt(tags[tags.length - 1]);
		} else if (event.key === 'Escape') {
			event.preventDefault();
			editing = false;
			draft = '';
		}
	}

	function onBlur() {
		if (draft === '') editing = false;
	}
</script>

<div class="flex flex-wrap items-center gap-1">
	<TagChips {tags} onRemove={removeTagAt} />
	{#if editing}
		<input
			bind:this={inputEl}
			bind:value={draft}
			onkeydown={onKeydown}
			onblur={onBlur}
			type="text"
			placeholder="tag"
			class="w-20 rounded-md border border-slate-300 px-1 py-0.5 text-xs"
		/>
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
