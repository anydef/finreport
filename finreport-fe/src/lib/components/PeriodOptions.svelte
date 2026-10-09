<script lang="ts">
	import { PERIOD_PRESETS, selectableMonths, selectableYears } from '$lib/period';

	interface Props {
		/** Offer the `custom` range entry (the dashboard has date inputs for it; /transactions does not). */
		allowCustom?: boolean;
		today?: Date;
	}

	let { allowCustom = true, today = new Date() }: Props = $props();

	const months = $derived(selectableMonths(today));
	const years = $derived(selectableYears(today));
	const presets = $derived(PERIOD_PRESETS.filter((p) => allowCustom || p.id !== 'custom'));
</script>

<optgroup label="Quick ranges">
	{#each presets as p (p.id)}
		<option value={p.id}>{p.label}</option>
	{/each}
</optgroup>
<optgroup label="Past months">
	{#each months as m (m.id)}
		<option value={m.id}>{m.label}</option>
	{/each}
</optgroup>
<optgroup label="Past years">
	{#each years as y (y.id)}
		<option value={y.id}>{y.label}</option>
	{/each}
</optgroup>
