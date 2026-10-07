<script lang="ts">
	import { goto } from '$app/navigation';
	import { createGraphqlClient } from '$lib/graphqlClient';
	import { LOGOUT_MUTATION } from '$lib/graphql/queries';
	import Button from '$lib/components/Button.svelte';
	import type { LayoutData } from './$types';

	let { data, children }: { data: LayoutData; children: import('svelte').Snippet } = $props();

	const tabs = [
		{ label: 'Dashboard', href: '/' },
		{ label: 'Transactions', href: '/transactions' },
		// Iteration 3 §6 WP0: added once here so WP-C's Recurring page and the
		// existing tabs never both edit this layout.
		{ label: 'Recurring', href: '/recurring' },
		// §10: added once here so WP5 (transactions) and WP6 (admin) never both
		// edit this layout. Route content under /admin/** is WP6's.
		{ label: 'Admin', href: '/admin' }
	];

	async function logout() {
		const client = createGraphqlClient(fetch);
		await client.mutation(LOGOUT_MUTATION, {}).toPromise();
		await goto('/login', { invalidateAll: true });
	}
</script>

<div class="min-h-screen bg-slate-50">
	<header class="border-b border-slate-200 bg-white">
		<div class="mx-auto flex max-w-6xl items-center justify-between px-4 py-3">
			<nav class="flex gap-1" aria-label="Main navigation">
				{#each tabs as tab (tab.href)}
					<a
						href={tab.href}
						class="focus-visible:outline-brand rounded-md px-3 py-1.5 text-sm font-medium text-slate-600 hover:bg-slate-100 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2"
					>
						{tab.label}
					</a>
				{/each}
			</nav>
			<div class="flex items-center gap-3 text-sm text-slate-600">
				<span>{data.user?.displayName ?? data.user?.username}</span>
				<Button variant="ghost" onclick={logout}>Log out</Button>
			</div>
		</div>
	</header>
	<main class="mx-auto max-w-6xl px-4 py-6">
		{@render children()}
	</main>
</div>
