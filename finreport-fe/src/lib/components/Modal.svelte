<script lang="ts">
	import type { Snippet } from 'svelte';

	interface Props {
		title: string;
		onclose: () => void;
		children: Snippet;
		class?: string;
		/** Phone width: fill the screen as a full-height sheet instead of a floating dialog. */
		sheet?: boolean;
	}

	let { title, onclose, children, class: className = '', sheet = false }: Props = $props();

	let dialog = $state<HTMLElement | null>(null);

	function onWindowKeydown(event: KeyboardEvent) {
		// A control inside (tag input, open dropdown) that already used Esc
		// has called preventDefault/stopPropagation; don't also close the dialog.
		if (event.key === 'Escape' && !event.defaultPrevented) onclose();
	}

	const FOCUSABLE =
		'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

	/** Keep Tab/Shift+Tab cycling inside the dialog. */
	function trapTab(event: KeyboardEvent) {
		if (event.key !== 'Tab' || !dialog) return;
		const items = [...dialog.querySelectorAll<HTMLElement>(FOCUSABLE)];
		if (items.length === 0) {
			event.preventDefault();
			return;
		}
		const first = items[0];
		const last = items[items.length - 1];
		const active = document.activeElement;
		if (event.shiftKey && (active === first || active === dialog)) {
			event.preventDefault();
			last.focus();
		} else if (!event.shiftKey && active === last) {
			event.preventDefault();
			first.focus();
		}
	}

	// Move focus into the dialog on open and hand it back to whatever opened it on close.
	$effect(() => {
		const opener = document.activeElement as HTMLElement | null;
		dialog?.focus();
		return () => {
			if (opener?.isConnected) opener.focus();
		};
	});
</script>

<svelte:window onkeydown={onWindowKeydown} />

<div
	class="fixed inset-0 z-50 flex items-center justify-center bg-slate-900/40 {sheet
		? 'p-0 sm:p-4'
		: 'p-4'}"
>
	<!-- eslint-disable-next-line svelte/no-static-element-interactions -->
	<div class="absolute inset-0" onclick={onclose} aria-hidden="true"></div>
	<div
		bind:this={dialog}
		role="dialog"
		aria-modal="true"
		aria-label={title}
		tabindex="-1"
		onkeydown={trapTab}
		class="relative flex w-full max-w-lg flex-col gap-4 overflow-y-auto bg-white p-5 shadow-xl outline-none {sheet
			? 'h-full max-h-full rounded-none sm:h-auto sm:max-h-[90vh] sm:rounded-lg'
			: 'max-h-[90vh] rounded-lg'} {className}"
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
