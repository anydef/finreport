<script lang="ts">
	/**
	 * One merchant's held transactions: the decision is made per merchant here
	 * (assign a category to the whole group, or accept the LLM's proposal when
	 * it is unanimous); the individual transactions open underneath for the
	 * odd one out. The keyless bucket cannot be addressed by any filter, so it
	 * offers no group action, only a way to the flat list.
	 */
	import type { Snippet } from 'svelte';
	import { formatAmount } from '$lib/format';
	import { isAssignable, proposalOf, reasonLabels } from '$lib/heldGroups';
	import type { HeldMerchantGroup } from '$lib/graphql/types';

	interface Props {
		group: HeldMerchantGroup;
		expanded: boolean;
		/** Link that opens (or, when already open, closes) this group. */
		toggleHref: string;
		/** Where the keyless bucket's transactions are reviewed one by one. */
		flatHref: string;
		busy?: boolean;
		onAssign: () => void;
		onAccept: (path: string) => void;
		children?: Snippet;
	}

	let {
		group,
		expanded,
		toggleHref,
		flatHref,
		busy = false,
		onAssign,
		onAccept,
		children
	}: Props = $props();

	const proposal = $derived(proposalOf(group));
	const assignable = $derived(isAssignable(group));
	const btn =
		'focus-visible:outline-brand cursor-pointer rounded-md px-3 py-1 text-xs font-medium focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 disabled:cursor-not-allowed disabled:opacity-50';
</script>

<div
	data-testid="held-group"
	data-group-key={group.counterpartyKey ?? ''}
	class="rounded-md border border-slate-200 bg-white"
>
	<div class="flex flex-wrap items-center justify-between gap-3 p-3">
		<div class="min-w-0">
			<p class="truncate text-sm font-medium text-slate-900">{group.displayName}</p>
			<p class="text-xs text-slate-500">
				<span data-testid="group-count">{group.heldCount} held</span>
				·
				<span data-testid="group-total">{formatAmount(group.totalAmount, group.currency)}</span>
				{#if reasonLabels(group.reviewReasons).length > 0}
					· {reasonLabels(group.reviewReasons).join(', ')}
				{/if}
			</p>
			{#if proposal.kind === 'unanimous'}
				<p class="text-xs text-slate-700" data-testid="group-proposal">
					Suggested category: <strong>{proposal.path}</strong>
				</p>
			{:else if proposal.kind === 'mixed'}
				<p class="text-xs text-amber-700" data-testid="group-proposal">
					Suggestions disagree: <strong>{proposal.path}</strong> for {proposal.votes} of {proposal.total}
				</p>
			{/if}
		</div>
		<div class="flex flex-wrap items-center gap-2">
			{#if assignable}
				{#if proposal.kind === 'unanimous'}
					<button
						type="button"
						disabled={busy}
						onclick={() => onAccept(proposal.path)}
						class="{btn} bg-brand hover:bg-brand/90 text-white"
					>
						Accept for all {group.heldCount}
					</button>
				{/if}
				<button
					type="button"
					disabled={busy}
					onclick={onAssign}
					class="{btn} {proposal.kind === 'unanimous'
						? 'bg-slate-100 text-slate-700 hover:bg-slate-200'
						: 'bg-brand hover:bg-brand/90 text-white'}"
				>
					Assign category to {group.heldCount === 1 ? 'it' : `all ${group.heldCount}`}…
				</button>
				<a
					href={toggleHref}
					data-sveltekit-noscroll
					aria-expanded={expanded}
					class="{btn} bg-slate-100 text-slate-700 hover:bg-slate-200"
				>
					{expanded ? 'Hide' : group.heldCount === 1 ? 'Show' : `Show ${group.heldCount}`}
				</a>
			{:else}
				<a href={flatHref} class="{btn} bg-slate-100 text-slate-700 hover:bg-slate-200">
					Review one by one
				</a>
			{/if}
		</div>
	</div>
	{#if expanded && children}
		<div class="flex flex-col gap-3 border-t border-slate-100 bg-slate-50/50 p-3">
			{@render children()}
		</div>
	{/if}
</div>
