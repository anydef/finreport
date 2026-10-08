<script lang="ts">
	/**
	 * One column header of a transaction table. Sortable (a `<button>` inside
	 * the `<th>`, `aria-sort` on the `<th>`) when `onsort` is given, plain text
	 * otherwise. The arrow is a glyph, not just a colour, and is hidden from
	 * assistive tech because `aria-sort` already announces the state.
	 */
	import { ariaSort } from '$lib/transactionSort';
	import type { TransactionSort, TransactionSortField } from '$lib/graphql/types';

	interface Props {
		label: string;
		field: TransactionSortField;
		sort: TransactionSort;
		onsort?: (field: TransactionSortField) => void;
		align?: 'left' | 'right';
	}

	let { label, field, sort, onsort, align = 'left' }: Props = $props();

	const current = $derived(ariaSort(sort, field));
	const arrow = $derived(current === 'ascending' ? '↑' : current === 'descending' ? '↓' : '↕');
</script>

<th
	class="py-2 pr-4 font-medium {align === 'right' ? 'text-right' : ''}"
	aria-sort={onsort ? current : undefined}
>
	{#if onsort}
		<button
			type="button"
			onclick={() => onsort(field)}
			class="focus-visible:outline-brand inline-flex items-center gap-1 rounded font-medium hover:text-slate-900 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 {current !==
			'none'
				? 'text-slate-900'
				: ''}"
		>
			{label}
			<span
				aria-hidden="true"
				data-testid="sort-indicator"
				class={current === 'none' ? 'text-slate-300' : ''}
			>
				{arrow}
			</span>
		</button>
	{:else}
		{label}
	{/if}
</th>
