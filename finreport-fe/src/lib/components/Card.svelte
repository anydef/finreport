<script lang="ts">
	import type { Snippet } from 'svelte';

	interface Props {
		title?: string;
		class?: string;
		children: Snippet;
	}

	let { title, class: className = '', children }: Props = $props();
</script>

<div class="rounded-lg border border-slate-200 bg-white p-4 shadow-sm {className}">
	{#if title}
		<h2 class="mb-3 text-sm font-semibold text-slate-500 uppercase">{title}</h2>
	{/if}
	{@render children()}
</div>
