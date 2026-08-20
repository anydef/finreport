<script lang="ts">
	import '../app.css';

	const tabs = [{ label: 'Transactions', href: '/transactions' }];

	let { children } = $props();
</script>

<nav class="tabs">
	{#each tabs as tab (tab.href)}
		<a href={tab.href}>{tab.label}</a>
	{/each}
</nav>

{@render children()}

<style>
	.tabs {
		display: flex;
		gap: 1rem;
		border-bottom: 1px solid #ddd;
		padding: 0.5rem 1rem;
	}

	.tabs a {
		padding: 0.5rem 0.75rem;
		text-decoration: none;
		color: inherit;
		border-radius: 0.25rem 0.25rem 0 0;
	}

	.tabs a:hover {
		background: #f0f0f0;
	}
</style>
