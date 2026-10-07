<script lang="ts">
	import type { Snippet } from 'svelte';

	interface Props {
		title: string;
		onclose: () => void;
		children: Snippet;
		class?: string;
	}

	let { title, onclose, children, class: className = '' }: Props = $props();

	function onBackdropKeydown(event: KeyboardEvent) {
		if (event.key === 'Escape') onclose();
	}
</script>

<svelte:window onkeydown={onBackdropKeydown} />

<div class="fixed inset-0 z-50 flex items-center justify-center bg-slate-900/40 p-4">
	<!-- eslint-disable-next-line svelte/no-static-element-interactions -->
	<div class="absolute inset-0" onclick={onclose} aria-hidden="true"></div>
	<div
		role="dialog"
		aria-modal="true"
		aria-label={title}
		class="relative flex max-h-[90vh] w-full max-w-lg flex-col gap-4 overflow-y-auto rounded-lg bg-white p-5 shadow-xl {className}"
	>
		<div class="flex items-center justify-between">
			<h2 class="text-sm font-semibold text-slate-900">{title}</h2>
			<button
				type="button"
				onclick={onclose}
				aria-label="Close dialog"
				class="focus-visible:outline-brand rounded-md px-2 py-1 text-slate-500 hover:bg-slate-100 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2"
			>
				✕
			</button>
		</div>
		{@render children()}
	</div>
</div>
