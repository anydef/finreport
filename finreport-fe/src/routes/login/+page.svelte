<script lang="ts">
	import { enhance } from '$app/forms';
	import Button from '$lib/components/Button.svelte';
	import Card from '$lib/components/Card.svelte';
	import Field from '$lib/components/Field.svelte';
	import type { ActionData } from './$types';

	let { form }: { form: ActionData } = $props();
	let submitting = $state(false);
</script>

<svelte:head>
	<title>Log in · finreport</title>
</svelte:head>

<div class="flex min-h-screen items-center justify-center bg-slate-50 px-4">
	<Card class="w-full max-w-sm" title="finreport">
		<form
			method="POST"
			use:enhance={() => {
				submitting = true;
				return async ({ update }) => {
					await update();
					submitting = false;
				};
			}}
			class="flex flex-col gap-4"
		>
			<Field label="Username" for="username">
				<input
					id="username"
					name="username"
					type="text"
					autocomplete="username"
					required
					class="rounded-md border-slate-300 text-sm"
				/>
			</Field>
			<Field label="Password" for="password">
				<input
					id="password"
					name="password"
					type="password"
					autocomplete="current-password"
					required
					class="rounded-md border-slate-300 text-sm"
				/>
			</Field>

			{#if form?.error}
				<p role="alert" class="text-sm text-[var(--color-spending)]">{form.error}</p>
			{/if}

			<Button type="submit" variant="primary" disabled={submitting} class="justify-center">
				{submitting ? 'Logging in…' : 'Log in'}
			</Button>
		</form>
	</Card>
</div>
