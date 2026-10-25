<script lang="ts">
	import { page } from '$app/state';
	import { visibleAdminTabs } from '$lib/adminAccess';
	import type { LayoutData } from './$types';

	let { data, children }: { data: LayoutData; children: import('svelte').Snippet } = $props();

	const subTabs = [
		{ label: 'Rules', href: '/admin/rules' },
		{ label: 'Categories', href: '/admin/categories' },
		{ label: 'Nicknames', href: '/admin/aliases' },
		{ label: 'Accounts', href: '/admin/accounts' },
		{ label: 'Users', href: '/admin/users' },
		{ label: 'Amazon', href: '/admin/amazon' }
	];

	// Cosmetic: the server-side gate is the control (see `$lib/adminAccess`).
	const shownTabs = $derived(visibleAdminTabs(subTabs, data.isAdmin));
</script>

<div class="flex flex-col gap-6">
	<nav class="flex gap-1 border-b border-slate-200" aria-label="Admin navigation">
		{#each shownTabs as tab (tab.href)}
			<a
				href={tab.href}
				aria-current={page.url.pathname.startsWith(tab.href) ? 'page' : undefined}
				class="focus-visible:outline-brand -mb-px rounded-t-md border-b-2 px-3 py-2 text-sm font-medium transition-colors focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 {page.url.pathname.startsWith(
					tab.href
				)
					? 'border-brand text-brand'
					: 'border-transparent text-slate-600 hover:text-slate-900'}"
			>
				{tab.label}
			</a>
		{/each}
	</nav>
	{@render children()}
</div>
