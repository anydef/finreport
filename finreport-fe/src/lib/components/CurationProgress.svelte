<!--
  Progress while curating: how far the uncategorised / needs-review counts have
  fallen since this tab first showed them, and (when the backend can say) how
  many rules were learned from the user's decisions.

  The baseline is per tab session, held in sessionStorage; the card says
  "since you opened this page" and never claims a daily figure. Storage can
  throw or be empty, and then the card simply shows no delta. A clean queue
  renders nothing: the "needs attention" strip above already says so.
-->
<script lang="ts">
	import { onMount } from 'svelte';
	import Card from './Card.svelte';
	import type { AttentionSummary } from '$lib/graphql/types';
	import {
		CURATION_METRICS,
		curationRows,
		resolveBaseline,
		rulesLearnedLine,
		type CurationMetric
	} from '$lib/curationView';

	interface Props {
		summary: AttentionSummary;
		/** Omitted until the backend exposes it; the line is then absent. */
		rulesLearned?: number;
		/** Mock mode only: first-view baselines, so a delta is reviewable. */
		seedBaselines?: Partial<Record<CurationMetric, number>>;
	}

	let { summary, rulesLearned, seedBaselines }: Props = $props();

	const counts = $derived({
		uncategorized: summary.uncategorized.count,
		needsReview: summary.needsReview.count
	});

	// Storage is browser-only, so baselines resolve after mount; until then (and
	// on the server) there is simply no delta.
	let baselines = $state<Record<CurationMetric, number | null>>({
		uncategorized: null,
		needsReview: null
	});

	onMount(() => {
		for (const metric of CURATION_METRICS) {
			baselines[metric] = resolveBaseline(metric, counts[metric], seedBaselines?.[metric]);
		}
	});

	const rows = $derived(curationRows({ counts, baselines }));
	const rulesLine = $derived(rulesLearnedLine(rulesLearned));
</script>

{#if rows.length > 0}
	<Card title="Your progress" class="py-3">
		<ul class="flex flex-wrap gap-x-8 gap-y-2" data-testid="curation-progress">
			{#each rows as row (row.metric)}
				<li data-testid="curation-{row.metric}">
					<span class="text-lg font-semibold text-slate-900 tabular-nums">{row.count}</span>
					<span class="text-sm text-slate-700">{row.label}</span>
					{#if row.delta}
						<span class="block text-xs text-slate-500" data-testid="curation-{row.metric}-delta"
							>{row.delta}</span
						>
					{/if}
				</li>
			{/each}
		</ul>
		{#if rulesLine}
			<p class="mt-2 text-xs text-slate-500" data-testid="curation-rules">{rulesLine}</p>
		{/if}
	</Card>
{/if}
