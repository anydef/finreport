<script lang="ts">
	import type { Granularity, PeriodSelection } from '$lib/period';
	import PeriodOptions from './PeriodOptions.svelte';
	import Field from './Field.svelte';

	interface Props {
		preset: PeriodSelection;
		startDate: string;
		endDate: string;
		granularity: Granularity;
		onchange: () => void;
	}

	let {
		preset = $bindable(),
		startDate = $bindable(),
		endDate = $bindable(),
		granularity = $bindable(),
		onchange
	}: Props = $props();
</script>

<form
	class="flex flex-wrap items-end gap-4"
	onsubmit={(e) => {
		e.preventDefault();
		onchange();
	}}
>
	<Field label="Period" for="period-preset">
		<select
			id="period-preset"
			bind:value={preset}
			onchange={() => onchange()}
			class="rounded-md border-slate-300 text-sm"
		>
			<PeriodOptions />
		</select>
	</Field>

	{#if preset === 'custom'}
		<Field label="Start date" for="period-start">
			<input
				id="period-start"
				type="date"
				bind:value={startDate}
				onchange={() => onchange()}
				class="rounded-md border-slate-300 text-sm"
			/>
		</Field>
		<Field label="End date" for="period-end">
			<input
				id="period-end"
				type="date"
				bind:value={endDate}
				onchange={() => onchange()}
				class="rounded-md border-slate-300 text-sm"
			/>
		</Field>
	{/if}

	<Field label="Granularity" for="period-granularity">
		<select
			id="period-granularity"
			bind:value={granularity}
			onchange={() => onchange()}
			class="rounded-md border-slate-300 text-sm"
		>
			<option value="DAY">Day</option>
			<option value="WEEK">Week</option>
			<option value="MONTH">Month</option>
		</select>
	</Field>
</form>
