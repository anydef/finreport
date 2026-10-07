<script lang="ts">
	import Card from './Card.svelte';
	import Badge from './Badge.svelte';
	import GoalProgressBar from './GoalProgressBar.svelte';
	import {
		bucketTone,
		currentBucket,
		describeGoalPeriod,
		formatMoney,
		goalTypeLabel,
		progressFraction,
		toneText,
		toneTextClass,
		totalLabel
	} from '$lib/goalsView';
	import type { Goal, GoalProgress } from '$lib/graphql/types';

	interface Props {
		goal: Goal;
		/** `undefined` while the goal's progress could not be loaded. */
		progress?: GoalProgress;
	}

	let { goal, progress }: Props = $props();

	const bucket = $derived(progress ? currentBucket(progress.buckets) : null);
	const tone = $derived(bucket ? bucketTone(goal.type, goal.amount, bucket) : 'neutral');
	const scopeSummary = $derived(
		[...goal.scope.categories.map((c) => c.name), ...goal.scope.tags.map((t) => `#${t}`)].join(', ')
	);
</script>

<Card>
	<a
		href="/goals/{goal.id}"
		class="focus-visible:outline-brand flex flex-col gap-3 rounded-md focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2"
	>
		<div class="flex items-start justify-between gap-2">
			<h3 class="text-base font-semibold text-slate-900">{goal.name}</h3>
			<Badge text={goalTypeLabel(goal.type)} variant="neutral" />
		</div>
		<p class="text-sm text-slate-500">
			{formatMoney(goal.amount, goal.currency)}
			{describeGoalPeriod(goal)}
		</p>
		{#if scopeSummary}
			<p class="truncate text-xs text-slate-400" title={scopeSummary}>{scopeSummary}</p>
		{/if}

		{#if bucket}
			<div class="flex flex-col gap-1.5">
				<div class="flex items-baseline justify-between text-sm">
					<span class="text-slate-500">
						{totalLabel(goal.type)} · {bucket.label}{bucket.inProgress ? ' (so far)' : ''}
					</span>
					<span class="font-medium {toneTextClass(tone)}">
						{formatMoney(bucket.total, goal.currency)}
					</span>
				</div>
				<GoalProgressBar
					fraction={progressFraction(bucket.total, goal.amount)}
					{tone}
					label="{goal.name} progress for {bucket.label}"
				/>
				<div class="flex items-baseline justify-between text-xs">
					<span class="font-medium {toneTextClass(tone)}">
						{toneText(goal.type, tone, bucket.inProgress)}
					</span>
					<span class="text-slate-500">
						{formatMoney(bucket.remaining, goal.currency)} remaining
					</span>
				</div>
			</div>
		{:else}
			<p class="text-sm text-slate-400">No progress available yet.</p>
		{/if}
	</a>
</Card>
