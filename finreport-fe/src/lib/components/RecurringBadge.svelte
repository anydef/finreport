<script lang="ts">
	/**
	 * Recurring-flag badge + toggle for a transaction row (§5): an auto-detected
	 * flag renders visibly distinct from a user override (Decision 2 — "you" vs
	 * "auto"). Clicking cycles `auto → recurring (you) → not recurring (you) →
	 * auto`, calling `setTransactionRecurring(true | false | null)` for the
	 * latter two → first transitions respectively.
	 */
	import Badge from './Badge.svelte';
	import type { RecurringInfo } from '$lib/graphql/types';

	interface Props {
		recurring: RecurringInfo;
		onToggle: (next: boolean | null) => void | Promise<void>;
	}

	let { recurring, onToggle }: Props = $props();

	const label = $derived(
		recurring.source === 'USER'
			? recurring.isRecurring
				? '↻ recurring · you'
				: 'not recurring · you'
			: recurring.isRecurring
				? '↻ recurring · auto'
				: 'not recurring · auto'
	);

	const variant = $derived(
		recurring.source === 'USER' ? (recurring.isRecurring ? 'success' : 'neutral') : 'info'
	);

	/** `auto → recurring(true) → not recurring(false) → auto(null)`. */
	function nextValue(): boolean | null {
		if (recurring.source === 'AUTO') return true;
		return recurring.isRecurring ? false : null;
	}

	function onclick() {
		onToggle(nextValue());
	}
</script>

<button type="button" class="inline-flex" {onclick} aria-label="Toggle recurring">
	<Badge text={label} {variant} />
</button>
