<script lang="ts">
	import { PERIOD_PRESETS, type Granularity, type PeriodPresetId } from '$lib/period';
	import Field from './Field.svelte';

	interface Props {
		preset: PeriodPresetId;
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
			{#each PERIOD_PRESETS as p (p.id)}
				<option value={p.id}>{p.label}</option>
			{/each}
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
