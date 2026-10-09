<script lang="ts">
	/**
	 * The netted figure for linked expenses, shown beside the period totals and
	 * never in place of them: the totals keep showing the money that actually
	 * moved in the period, this says how much of the linked spending was paid
	 * back (wherever and whenever the reimbursement landed). Renders nothing
	 * when nothing in range is linked.
	 */
	import { summaryLine } from '$lib/reimbursement';
	import type { ReimbursementSummary } from '$lib/graphql/types';

	interface Props {
		summary: ReimbursementSummary | undefined;
		currency: string;
	}

	let { summary, currency }: Props = $props();

	const line = $derived(summaryLine(summary, currency));
</script>

{#if line}
	<p
		data-testid="reimbursement-note"
		class="rounded-md border border-slate-200 bg-slate-50 px-3 py-2 text-sm text-slate-700"
	>
		{line}
		<span class="text-slate-500">Period totals still show the money that actually moved.</span>
	</p>
{/if}
