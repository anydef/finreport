<script lang="ts">
	import Modal from '$lib/components/Modal.svelte';
	import { counterpartyLabel } from '$lib/displayNames';
	import type { Category, Transaction } from '$lib/graphql/types';
	import { formatAmount as formatDisplayAmount } from '$lib/format';
	import { formatAmount, isZeroAmount, parseAmount, remainder } from '$lib/splitMath';

	interface Props {
		transaction: Transaction;
		categories: Category[];
		onSubmit: (parts: { amount: string; categorySlug: string }[]) => Promise<void>;
		onUnsplit: () => Promise<void>;
		onClose: () => void;
	}

	let { transaction, categories, onSubmit, onUnsplit, onClose }: Props = $props();

	interface PartRow {
		amount: string;
		categorySlug: string;
	}

	function initialParts(): PartRow[] {
		const existing = transaction.splits ?? [];
		if (existing.length > 0) {
			return existing.map((s) => ({ amount: s.amount, categorySlug: s.category.slug }));
		}
		return [
			{ amount: '', categorySlug: '' },
			{ amount: '', categorySlug: '' }
		];
	}

	let parts = $state<PartRow[]>(initialParts());
	let submitting = $state(false);
	let errorMessage = $state('');

	/** Same local grouping as `ReviewCard` — intentionally not the shared
	 * `CategoryPicker` (WP5-owned, not usable from this package yet). */
	function groupedOptions(cats: Category[]) {
		const byId = new Map(cats.map((c) => [c.id, c]));
		function pathOf(cat: Category): string {
			return cat.parentId && byId.has(cat.parentId)
				? `${pathOf(byId.get(cat.parentId)!)} / ${cat.name}`
				: cat.name;
		}
		const groups = new Map<string, { slug: string; label: string }[]>();
		for (const cat of cats) {
			if (cat.archived) continue;
			const top =
				cat.parentId && byId.has(cat.parentId)
					? pathOf(byId.get(cat.parentId)!).split(' / ')[0]
					: cat.name;
			const list = groups.get(top) ?? [];
			list.push({ slug: cat.slug, label: pathOf(cat) });
			groups.set(top, list);
		}
		return [...groups.entries()].map(([groupLabel, options]) => ({
			groupLabel,
			options: options.sort((a, b) => a.label.localeCompare(b.label))
		}));
	}

	const optionGroups = groupedOptions(categories);

	function addPart() {
		parts = [...parts, { amount: '', categorySlug: '' }];
	}

	function removePart(index: number) {
		parts = parts.filter((_, i) => i !== index);
	}

	/** `null` when any part's amount doesn't parse as a decimal yet. */
	const remainderScaled = $derived.by(() => {
		try {
			const total = parseAmount(transaction.amount);
			const scaledParts = parts.map((p) => parseAmount(p.amount || '0'));
			return remainder(total, scaledParts);
		} catch {
			return null;
		}
	});

	const allCategoriesPicked = $derived(parts.every((p) => p.categorySlug));
	const canSubmit = $derived(
		parts.length >= 2 &&
			allCategoriesPicked &&
			remainderScaled !== null &&
			isZeroAmount(remainderScaled)
	);

	function splitTheRemainder() {
		if (remainderScaled === null || parts.length === 0) return;
		const last = parts[parts.length - 1];
		const lastScaled = (() => {
			try {
				return parseAmount(last.amount || '0');
			} catch {
				return 0n;
			}
		})();
		parts = parts.map((p, i) =>
			i === parts.length - 1 ? { ...p, amount: formatAmount(lastScaled + remainderScaled) } : p
		);
	}

	async function submit() {
		if (!canSubmit) return;
		submitting = true;
		errorMessage = '';
		try {
			await onSubmit(parts.map((p) => ({ amount: p.amount, categorySlug: p.categorySlug })));
			onClose();
		} catch (err) {
			errorMessage = err instanceof Error ? err.message : 'Failed to save split';
		} finally {
			submitting = false;
		}
	}

	async function unsplit() {
		submitting = true;
		errorMessage = '';
		try {
			await onUnsplit();
			onClose();
		} catch (err) {
			errorMessage = err instanceof Error ? err.message : 'Failed to remove split';
		} finally {
			submitting = false;
		}
	}
</script>

<Modal title="Split transaction" onclose={onClose}>
	<p class="text-xs text-slate-500">
		Total: {formatDisplayAmount(transaction.amount, transaction.currency)}
		{#if counterpartyLabel(transaction)}· {counterpartyLabel(transaction)}{/if}
	</p>

	<div class="flex flex-col gap-2">
		{#each parts as part, i (i)}
			<div class="flex items-center gap-2">
				<select
					bind:value={part.categorySlug}
					aria-label={`Part ${i + 1} category`}
					class="flex-1 rounded-md border-slate-300 text-sm"
				>
					<option value="">Select a category…</option>
					{#each optionGroups as group (group.groupLabel)}
						<optgroup label={group.groupLabel}>
							{#each group.options as option (option.slug)}
								<option value={option.slug}>{option.label}</option>
							{/each}
						</optgroup>
					{/each}
				</select>
				<input
					type="text"
					inputmode="decimal"
					bind:value={part.amount}
					aria-label={`Part ${i + 1} amount`}
					placeholder="0.00"
					class="w-28 rounded-md border-slate-300 text-sm"
				/>
				<button
					type="button"
					disabled={parts.length <= 2}
					onclick={() => removePart(i)}
					aria-label={`Remove part ${i + 1}`}
					class="focus-visible:outline-brand rounded-md px-2 py-1 text-slate-500 hover:bg-slate-100 focus-visible:outline focus-visible:outline-2 disabled:opacity-30"
				>
					✕
				</button>
			</div>
		{/each}
	</div>

	<div class="flex items-center justify-between gap-2">
		<button
			type="button"
			onclick={addPart}
			class="focus-visible:outline-brand rounded-md bg-slate-100 px-2 py-1 text-xs font-medium text-slate-700 hover:bg-slate-200 focus-visible:outline focus-visible:outline-2"
		>
			+ Add part
		</button>
		<button
			type="button"
			onclick={splitTheRemainder}
			disabled={remainderScaled === null || isZeroAmount(remainderScaled)}
			class="focus-visible:outline-brand rounded-md bg-slate-100 px-2 py-1 text-xs font-medium text-slate-700 hover:bg-slate-200 focus-visible:outline focus-visible:outline-2 disabled:opacity-50"
		>
			Split the remainder
		</button>
	</div>

	<p
		class="text-sm font-medium {remainderScaled !== null && isZeroAmount(remainderScaled)
			? 'text-[var(--color-income)]'
			: 'text-[var(--color-spending)]'}"
	>
		Remainder: {remainderScaled !== null
			? formatDisplayAmount(formatAmount(remainderScaled), transaction.currency)
			: '—'}
	</p>

	{#if errorMessage}
		<p role="alert" class="text-xs text-[var(--color-spending)]">{errorMessage}</p>
	{/if}

	<div class="flex justify-between gap-2">
		{#if (transaction.splits ?? []).length > 0}
			<button
				type="button"
				disabled={submitting}
				onclick={unsplit}
				class="focus-visible:outline-brand rounded-md bg-slate-100 px-3 py-1.5 text-sm font-medium text-slate-700 hover:bg-slate-200 focus-visible:outline focus-visible:outline-2 disabled:opacity-50"
			>
				Unsplit
			</button>
		{:else}
			<span></span>
		{/if}
		<button
			type="button"
			disabled={!canSubmit || submitting}
			onclick={submit}
			class="focus-visible:outline-brand bg-brand hover:bg-brand/90 rounded-md px-3 py-1.5 text-sm font-medium text-white focus-visible:outline focus-visible:outline-2 disabled:opacity-50"
		>
			Save split
		</button>
	</div>
</Modal>
