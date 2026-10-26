<script lang="ts">
	/**
	 * `/admin/c24`: import C24 CSV account exports.
	 *
	 * C24 rows are transactions, so there is nothing to review: upload, then
	 * report what changed. Not admin-only: it is the caller's own statement.
	 * The file text is read in the browser, sent once and never logged,
	 * echoed or previewed; rejected files are named by position and the
	 * server's value-free message.
	 */
	import Button from '$lib/components/Button.svelte';
	import Card from '$lib/components/Card.svelte';
	import { c24Outcome, uploadErrorMessage } from '$lib/c24UploadView';
	import type { C24Outcome } from '$lib/c24UploadView';
	import { createGraphqlClient } from '$lib/graphqlClient';
	import { UPLOAD_C24_TRANSACTIONS_MUTATION } from '$lib/graphql/adminC24Queries';
	import type { C24UploadResult } from '$lib/graphql/types';

	let fileInput = $state<HTMLInputElement | null>(null);
	let busy = $state(false);
	let errorMessage = $state<string | null>(null);
	let outcome = $state<C24Outcome | null>(null);

	async function upload() {
		const picked = Array.from(fileInput?.files ?? []);
		if (picked.length === 0) {
			errorMessage = 'Choose one or more CSV files first.';
			return;
		}
		busy = true;
		errorMessage = null;
		outcome = null;
		try {
			const files = await Promise.all(picked.map((f) => f.text()));
			const result = await createGraphqlClient(fetch)
				.mutation(UPLOAD_C24_TRANSACTIONS_MUTATION, { files })
				.toPromise();
			if (result.error || !result.data) {
				const first = result.error?.graphQLErrors[0];
				errorMessage = uploadErrorMessage(
					first?.extensions?.code as string | undefined,
					first?.message
				);
			} else {
				outcome = c24Outcome(
					result.data.uploadC24Transactions as C24UploadResult,
					picked.map((f) => f.name)
				);
				if (fileInput) fileInput.value = '';
			}
		} catch {
			errorMessage = 'Could not read or upload the files. Try again.';
		} finally {
			busy = false;
		}
	}

	const toneClass = {
		good: 'bg-emerald-50 text-emerald-900',
		warn: 'bg-amber-50 text-amber-900',
		bad: 'bg-red-50 text-red-800'
	};
</script>

<div class="flex flex-col gap-6">
	<h1 class="text-lg font-semibold text-slate-900">C24 transactions</h1>

	<Card title="Upload account export">
		<p class="mb-3 text-sm text-slate-600">
			Choose one or more CSV files exported from C24. The imported accounts are linked to you.
			Uploading the same period again is safe: rows already imported are skipped, so overlapping
			exports never duplicate.
		</p>
		<div class="flex flex-col gap-3 sm:flex-row sm:items-center">
			<input
				bind:this={fileInput}
				type="file"
				accept=".csv,text/csv"
				multiple
				disabled={busy}
				class="min-w-0 text-sm text-slate-700"
				aria-label="C24 CSV export files"
			/>
			<Button variant="primary" onclick={upload} disabled={busy}>
				{busy ? 'Uploading…' : 'Upload'}
			</Button>
		</div>
		<p class="mt-2 text-xs text-slate-500">
			A file with an unrecognised header or an unreadable value is rejected whole, never row by row,
			so a statement is never half-imported. Other files in the same upload still import.
		</p>
		{#if errorMessage}
			<p class="mt-3 text-sm text-red-600" role="alert">{errorMessage}</p>
		{/if}
		{#if outcome}
			<div class="mt-3 rounded-md px-3 py-2 text-sm {toneClass[outcome.tone]}" role="status">
				{#each outcome.lines as line (line)}
					<p>{line}</p>
				{/each}
			</div>
			{#if outcome.rejected.length > 0}
				<ul class="mt-3 divide-y divide-slate-100 rounded-md border border-red-200 text-sm">
					{#each outcome.rejected as file (file.label)}
						<li class="flex flex-col gap-0.5 px-3 py-2 sm:flex-row sm:gap-3">
							<span class="font-medium break-all text-slate-800">{file.label}</span>
							<span class="text-red-800">{file.message}</span>
						</li>
					{/each}
				</ul>
			{/if}
		{/if}
	</Card>
</div>
