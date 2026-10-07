<script lang="ts">
	import type { Snippet } from 'svelte';

	interface Props {
		label: string;
		for: string;
		class?: string;
		children: Snippet;
	}

	let { label, for: htmlFor, class: className = '', children }: Props = $props();
</script>

<label for={htmlFor} class="flex flex-col gap-1 text-sm font-medium text-slate-700 {className}">
	{label}
	{@render children()}
</label>
