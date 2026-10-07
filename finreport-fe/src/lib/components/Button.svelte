<script lang="ts">
	import type { Snippet } from 'svelte';
	import type { HTMLButtonAttributes } from 'svelte/elements';

	interface Props extends HTMLButtonAttributes {
		variant?: 'primary' | 'secondary' | 'ghost';
		children: Snippet;
	}

	let { variant = 'secondary', children, class: className = '', ...rest }: Props = $props();

	const base =
		'inline-flex items-center justify-center gap-2 rounded-md px-3 py-1.5 text-sm font-medium transition-colors focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-brand disabled:opacity-50 disabled:pointer-events-none';
	const variants = {
		primary: 'bg-brand text-white hover:bg-brand/90',
		secondary: 'bg-slate-100 text-slate-900 hover:bg-slate-200',
		ghost: 'text-slate-600 hover:bg-slate-100'
	};
</script>

<button class="{base} {variants[variant]} {className}" {...rest}>
	{@render children()}
</button>
