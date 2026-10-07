<script lang="ts">
	import { toneBarClass, type GoalTone } from '$lib/goalsView';

	interface Props {
		/** `0..1`, from `progressFraction`. */
		fraction: number;
		tone: GoalTone;
		label: string;
	}

	let { fraction, tone, label }: Props = $props();
</script>

<div
	role="progressbar"
	aria-label={label}
	aria-valuemin={0}
	aria-valuemax={100}
	aria-valuenow={Math.round(fraction * 100)}
	class="h-2 w-full overflow-hidden rounded-full bg-slate-100"
>
	<div class="h-full rounded-full {toneBarClass(tone)}" style="width: {fraction * 100}%"></div>
</div>
