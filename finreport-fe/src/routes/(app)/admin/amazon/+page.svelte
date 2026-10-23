<script lang="ts">
	/**
	 * `/admin/amazon`: upload Amazon order-history CSVs and review how the
	 * orders line up with bank charges.
	 *
	 * Uploading records evidence only. It does not split, recategorise or
	 * otherwise change any transaction. Applying a clean match does: it splits
	 * the order's bank charge into one part per item (or, for a single item, sets
	 * its category), in categories the user picks, behind an explicit confirmation, and can be undone. The matcher
	 * calls a match clean only when exactly one assignment of items to charges
	 * reconciles, so most of this page is the cases it declined, grouped by
	 * why and worded by the matcher itself. The server re-checks everything on
	 * apply; the page only gathers category choices.
	 */
	import { invalidateAll } from '$app/navigation';
	import Badge from '$lib/components/Badge.svelte';
	import Button from '$lib/components/Button.svelte';
	import Card from '$lib/components/Card.svelte';
	import SearchMenu from '$lib/components/SearchMenu.svelte';
	import {
		appliedIntro,
		applicationLabel,
		applyActionLabel,
		applyBlocker,
		applyProposals,
		applyWarning,
		buildApplyInput,
		buildReview,
		categoryPickerGroups,
		chargeLabel,
		confirmActionLabel,
		describeOutcomes,
		emptyChoices,
		intactNote,
		proposalReasonText,
		proposalSourceLabel,
		readyIntro,
		reviewHeadline,
		uploadOutcome
	} from '$lib/amazonView';
	import type {
		ActionOutcome,
		ApplyChoices,
		MatchRow,
		ProposalApplication,
		UploadOutcome
	} from '$lib/amazonView';
	import { formatAmount, formatDisplayDate } from '$lib/format';
	import { createGraphqlClient } from '$lib/graphqlClient';
	import {
		APPLY_PURCHASE_ORDER_SPLIT_MUTATION,
		PROPOSE_PURCHASE_ORDER_CATEGORIES_MUTATION,
		UNDO_PURCHASE_ORDER_SPLIT_MUTATION,
		UPLOAD_PURCHASE_ORDERS_MUTATION
	} from '$lib/graphql/adminAmazonQueries';
	import type {
		PurchaseOrderApplyResult,
		PurchaseOrderItemProposal,
		PurchaseOrderUploadResult
	} from '$lib/graphql/types';
	import type { PageData } from './$types';

	let { data }: { data: PageData } = $props();

	const review = $derived(buildReview(data.report));
	const optionGroups = $derived(categoryPickerGroups(data.categories));

	// -- Applying and undoing: the only actions here that change transactions.
	let choices = $state<Record<string, ApplyChoices>>({});
	let confirming = $state<string | null>(null);
	let acting = $state<string | null>(null);
	let actionOutcome = $state<(ActionOutcome & { orderId: string }) | null>(null);

	// Read-only (safe in a template); writes happen in `setChoice`.
	function choicesFor(row: MatchRow): ApplyChoices {
		return choices[row.orderId] ?? emptyChoices(row.order);
	}

	function setChoice(row: MatchRow, itemIndex: number, slug: string) {
		const current = choicesFor(row);
		const itemCategories = [...current.itemCategories];
		itemCategories[itemIndex] = slug;
		choices[row.orderId] = { ...current, itemCategories };
	}

	// -- Proposals: suggestions only. They fill empty pickers; applying stays
	// a separate, confirmed step. A picker is "chosen" when its slug is
	// non-empty, so a user's pick is never overwritten.
	let suggestions = $state<Record<string, ProposalApplication>>({});
	const knownSlugs = $derived(new Set(optionGroups.flatMap((g) => g.options.map((o) => o.slug))));

	async function proposeOrder(row: MatchRow) {
		acting = row.orderId;
		actionOutcome = null;
		try {
			const result = await createGraphqlClient(fetch)
				.mutation(PROPOSE_PURCHASE_ORDER_CATEGORIES_MUTATION, { orderId: row.orderId })
				.toPromise();
			if (result.error) {
				actionOutcome = {
					orderId: row.orderId,
					tone: 'bad',
					lines: [result.error.graphQLErrors[0]?.message ?? 'Could not get suggestions.']
				};
			} else {
				const merged = applyProposals(
					choicesFor(row).itemCategories,
					result.data.proposePurchaseOrderCategories as PurchaseOrderItemProposal[],
					knownSlugs
				);
				choices[row.orderId] = { ...choicesFor(row), itemCategories: merged.itemCategories };
				suggestions[row.orderId] = merged;
			}
		} finally {
			acting = null;
		}
	}

	async function applyOrder(row: MatchRow) {
		acting = row.orderId;
		actionOutcome = null;
		try {
			const result = await createGraphqlClient(fetch)
				.mutation(APPLY_PURCHASE_ORDER_SPLIT_MUTATION, {
					input: buildApplyInput(row, choicesFor(row))
				})
				.toPromise();
			if (result.error) {
				actionOutcome = {
					orderId: row.orderId,
					tone: 'bad',
					lines: [result.error.graphQLErrors[0]?.message ?? 'Could not apply.']
				};
			} else {
				actionOutcome = {
					orderId: row.orderId,
					...describeOutcomes(
						result.data.applyPurchaseOrderSplit as PurchaseOrderApplyResult,
						'apply'
					)
				};
				await invalidateAll();
			}
		} finally {
			acting = null;
			confirming = null;
		}
	}

	async function undoOrder(row: MatchRow) {
		acting = row.orderId;
		actionOutcome = null;
		try {
			const result = await createGraphqlClient(fetch)
				.mutation(UNDO_PURCHASE_ORDER_SPLIT_MUTATION, { orderId: row.orderId })
				.toPromise();
			if (result.error) {
				actionOutcome = {
					orderId: row.orderId,
					tone: 'bad',
					lines: [result.error.graphQLErrors[0]?.message ?? 'Could not undo.']
				};
			} else {
				actionOutcome = {
					orderId: row.orderId,
					...describeOutcomes(
						result.data.undoPurchaseOrderSplit as PurchaseOrderApplyResult,
						'undo'
					)
				};
				await invalidateAll();
			}
		} finally {
			acting = null;
		}
	}

	let fileInput = $state<HTMLInputElement | null>(null);
	let busy = $state(false);
	let errorMessage = $state<string | null>(null);
	let outcome = $state<UploadOutcome | null>(null);

	async function upload() {
		const picked = Array.from(fileInput?.files ?? []);
		if (picked.length === 0) {
			errorMessage = 'Choose one or more CSV files first.';
			return;
		}
		busy = true;
		errorMessage = null;
		outcome = null;
		try {
			const files = await Promise.all(picked.map((f) => f.text()));
			const result = await createGraphqlClient(fetch)
				.mutation(UPLOAD_PURCHASE_ORDERS_MUTATION, { files })
				.toPromise();
			if (result.error) {
				errorMessage =
					result.error.graphQLErrors[0]?.message ?? 'Could not upload the files. Try again.';
			} else {
				outcome = uploadOutcome(result.data.uploadPurchaseOrders as PurchaseOrderUploadResult);
				if (fileInput) fileInput.value = '';
				await invalidateAll();
			}
		} finally {
			busy = false;
		}
	}

	const toneClass = {
		good: 'bg-emerald-50 text-emerald-900',
		warn: 'bg-amber-50 text-amber-900',
		bad: 'bg-red-50 text-red-800'
	};
</script>

{#snippet suggestionNote(row: MatchRow, i: number, chosen: string)}
	{@const filled = suggestions[row.orderId]?.filled[i]}
	{@const other = suggestions[row.orderId]?.notFilled[i]}
	{#if filled && filled.categorySlug === chosen}
		{@const label = proposalSourceLabel(filled.source)}
		<Badge text={label.text} variant={label.variant} />
	{:else if other?.source === 'NONE'}
		<span class="flex flex-wrap items-center justify-end gap-1 text-xs text-slate-500">
			<Badge text="No suggestion" variant="neutral" />
			{proposalReasonText(other.reason)}
		</span>
	{:else if other && other.categorySlug && other.categorySlug !== chosen}
		<span class="text-xs text-slate-500">Kept your choice (suggestion: {other.categorySlug})</span>
	{/if}
{/snippet}

<div class="flex flex-col gap-6">
	<h1 class="text-lg font-semibold text-slate-900">Amazon orders</h1>

	<Card title="Upload order history">
		<p class="mb-3 text-sm text-slate-600">
			Choose the order-history CSV you exported from Amazon. Uploading records the orders as
			evidence. <strong>Uploading does not split or change any transaction;</strong> applying a match
			below is a separate step you confirm. Your delivery address columns are ignored: they are never
			stored or logged.
		</p>
		<div class="flex flex-wrap items-center gap-3">
			<input
				bind:this={fileInput}
				type="file"
				accept=".csv,text/csv"
				multiple
				disabled={busy}
				class="text-sm text-slate-700"
				aria-label="Amazon order history CSV files"
			/>
			<Button variant="primary" onclick={upload} disabled={busy}>
				{busy ? 'Uploading…' : 'Upload'}
			</Button>
		</div>
		<p class="mt-2 text-xs text-slate-500">
			A file with an unrecognised header or an unreadable value is rejected whole, so an order is
			never stored with items missing. Other files in the same upload still import. Uploading the
			same file again changes nothing.
		</p>
		{#if errorMessage}
			<p class="mt-3 text-sm text-red-600" role="alert">{errorMessage}</p>
		{/if}
		{#if outcome}
			<div class="mt-3 rounded-md px-3 py-2 text-sm {toneClass[outcome.tone]}" role="status">
				{#each outcome.lines as line (line)}
					<p>{line}</p>
				{/each}
			</div>
		{/if}
	</Card>

	{#if data.error}
		<p class="text-sm text-red-600">Failed to load the order matches.</p>
	{/if}

	<p class="text-sm text-slate-700" role="status">{reviewHeadline(review)}</p>

	{#if !review.isEmpty}
		<Card title="Need review ({review.reviewCount})">
			{#if review.reviewGroups.length === 0}
				<p class="text-sm text-slate-500">Nothing needs review.</p>
			{/if}
			<div class="flex flex-col gap-5">
				{#each review.reviewGroups as group (group.code)}
					<section aria-label={group.title}>
						<h3 class="text-sm font-semibold text-slate-800">
							{group.title} <span class="font-normal text-slate-500">({group.rows.length})</span>
						</h3>
						<p class="mb-2 text-xs text-slate-500">{group.hint}</p>
						<ul class="divide-y divide-slate-100 rounded-md border border-slate-200">
							{#each group.rows as row (row.orderId)}
								<li class="flex flex-col gap-1 px-3 py-2">
									<div class="flex flex-wrap items-center justify-between gap-2">
										<span class="font-mono text-sm text-slate-800">{row.orderId}</span>
										<span class="text-xs text-slate-500">
											{formatDisplayDate(row.orderDate)} · order total {formatAmount(
												row.total,
												row.currency
											)} · charged {row.chargedTotal.toFixed(2)}
											{row.currency}
										</span>
									</div>
									{#each row.reasons as reason (reason)}
										<p class="text-sm text-amber-900">{reason}</p>
									{/each}
									{#if row.fuzzy}
										<Badge text="matched on amount and date only" variant="warning" />
									{/if}
									<ul class="text-xs text-slate-600" aria-label="Bank charges for {row.orderId}">
										{#each row.charges as charge (charge.transactionId)}
											<li>
												{formatDisplayDate(charge.bookingDate)}
												{formatAmount(charge.amount, charge.currency)}
												<span class="text-slate-400">{chargeLabel(charge)}</span>
											</li>
										{/each}
									</ul>
									<details class="text-xs text-slate-600">
										<summary class="cursor-pointer">
											{row.itemCount}
											{row.itemCount === 1 ? 'item' : 'items'}
										</summary>
										<ul class="mt-1">
											{#each row.order.items as item (item.asin + item.title)}
												<li class="flex justify-between gap-2">
													<span>{item.title}</span>
													<span>{formatAmount(item.lineTotal, row.currency)}</span>
												</li>
											{/each}
										</ul>
									</details>
								</li>
							{/each}
						</ul>
					</section>
				{/each}
			</div>
		</Card>

		<Card title="Match cleanly ({review.ready.length})">
			<p class="mb-2 text-sm text-slate-600">
				{readyIntro()}
			</p>
			<ul class="flex flex-col gap-3" aria-label="Orders that match cleanly">
				{#each review.ready as row (row.orderId)}
					{@const rowChoices = choicesFor(row)}
					{@const blocker = applyBlocker(row, rowChoices)}
					<li class="rounded-md border border-slate-200 px-3 py-2 text-sm">
						<div class="flex flex-wrap items-center justify-between gap-2">
							<span class="font-mono text-slate-800">{row.orderId}</span>
							<span class="text-xs text-slate-500">
								{formatDisplayDate(row.orderDate)} · {row.itemCount}
								{row.itemCount === 1 ? 'item' : 'items'} · {formatAmount(row.total, row.currency)}
							</span>
						</div>
						<ul
							class="mt-2 flex flex-col gap-1"
							aria-label="Choose a category for each item of {row.orderId}"
						>
							{#each row.order.items as item, i (item.asin + i)}
								<li class="flex flex-wrap items-center justify-between gap-2">
									<span class="text-slate-700">
										{item.title}
										<span class="text-xs text-slate-500"
											>{formatAmount(item.lineTotal, row.currency)}</span
										>
									</span>
									<div class="flex flex-col items-end gap-1">
										<SearchMenu
											groups={optionGroups}
											value={rowChoices.itemCategories[i] || null}
											onchange={(slug) => setChoice(row, i, slug)}
											label="Category for {item.title}"
											placeholder="Choose a category…"
											id="amazon-{row.orderId}-{i}"
											disabled={acting !== null}
										/>
										{@render suggestionNote(row, i, rowChoices.itemCategories[i])}
									</div>
								</li>
							{/each}
						</ul>
						{#if confirming === row.orderId}
							<div
								class="mt-3 rounded-md bg-amber-50 px-3 py-2 text-amber-900"
								role="alertdialog"
								aria-label="Confirm applying {row.orderId}"
							>
								<p>{applyWarning(row)}</p>
								<div class="mt-2 flex gap-2">
									<Button
										variant="primary"
										onclick={() => applyOrder(row)}
										disabled={acting !== null}
									>
										{acting === row.orderId ? 'Applying…' : confirmActionLabel(row)}
									</Button>
									<Button
										variant="ghost"
										onclick={() => (confirming = null)}
										disabled={acting !== null}
									>
										Cancel
									</Button>
								</div>
							</div>
						{:else}
							<div class="mt-3 flex flex-wrap items-center gap-3">
								<Button
									variant="ghost"
									onclick={() => proposeOrder(row)}
									disabled={acting !== null}
								>
									{acting === row.orderId ? 'Working…' : 'Suggest categories'}
								</Button>
								<Button
									variant="secondary"
									onclick={() => (confirming = row.orderId)}
									disabled={blocker !== null || acting !== null}
								>
									{applyActionLabel(row)}
								</Button>
								{#if blocker}
									<span class="text-xs text-slate-500">{blocker}</span>
								{/if}
							</div>
						{/if}
						{#if actionOutcome?.orderId === row.orderId}
							<div
								class="mt-2 rounded-md px-3 py-2 text-sm {toneClass[actionOutcome.tone]}"
								role="status"
							>
								{#each actionOutcome.lines as line (line)}
									<p>{line}</p>
								{/each}
							</div>
						{/if}
					</li>
				{:else}
					<li class="py-2 text-sm text-slate-500">None waiting.</li>
				{/each}
			</ul>
		</Card>

		{#if review.applied.length > 0}
			<Card title="Applied ({review.applied.length})">
				<p class="mb-2 text-sm text-slate-600">
					{appliedIntro(review.applied)}
				</p>
				<ul class="flex flex-col gap-3" aria-label="Applied orders">
					{#each review.applied as row (row.orderId)}
						<li class="rounded-md border border-slate-200 px-3 py-2 text-sm">
							<div class="flex flex-wrap items-center justify-between gap-2">
								<span class="font-mono text-slate-800">{row.orderId}</span>
								<Button
									variant="secondary"
									onclick={() => undoOrder(row)}
									disabled={acting !== null}
								>
									{acting === row.orderId ? 'Undoing…' : 'Undo'}
								</Button>
							</div>
							{#each row.applications as app (app.transactionId)}
								<p class="mt-2 text-xs font-medium text-slate-700">{applicationLabel(app)}</p>
								<ul
									class="mt-1 text-xs text-slate-600"
									aria-label="Parts applied to {app.transactionId}"
								>
									{#each app.parts as part, i (i)}
										<li class="flex justify-between gap-2">
											<span
												>{part.label} <span class="text-slate-400">{part.categorySlug}</span></span
											>
											<span>{formatAmount(part.amount, row.currency)}</span>
										</li>
									{/each}
									{#if intactNote(app)}
										<li class="mt-1 text-amber-900">{intactNote(app)}</li>
									{/if}
								</ul>
							{/each}
							{#if actionOutcome?.orderId === row.orderId}
								<div
									class="mt-2 rounded-md px-3 py-2 {toneClass[actionOutcome.tone]}"
									role="status"
								>
									{#each actionOutcome.lines as line (line)}
										<p>{line}</p>
									{/each}
								</div>
							{/if}
						</li>
					{/each}
				</ul>
			</Card>
		{/if}

		{#if review.noCharge.length > 0}
			<Card title="Orders with no bank charge ({review.noCharge.length})">
				<p class="mb-2 text-sm text-slate-600">
					Possibly paid by gift card, not charged yet, or older than the imported bank history. They
					stay stored and are matched again if a charge appears.
				</p>
				<ul class="divide-y divide-slate-100" aria-label="Orders with no bank charge">
					{#each review.noCharge as o (o.orderId)}
						<li class="flex flex-wrap items-center justify-between gap-2 py-2 text-sm">
							<span class="font-mono text-slate-800">{o.orderId}</span>
							<span class="text-xs text-slate-500">
								{formatDisplayDate(o.orderDate)} · {formatAmount(o.total, o.currency)}
							</span>
						</li>
					{/each}
				</ul>
			</Card>
		{/if}

		{#if review.unclaimed.length > 0}
			<Card title="Amazon charges no order explains ({review.unclaimed.length})">
				<p class="mb-2 text-sm text-slate-600">
					The order may be missing from what you uploaded. Upload an export covering that date.
				</p>
				<ul class="divide-y divide-slate-100" aria-label="Unclaimed Amazon charges">
					{#each review.unclaimed as c (c.transactionId)}
						<li class="flex flex-wrap items-center justify-between gap-2 py-2 text-sm">
							<span class="text-slate-800">{chargeLabel(c)}</span>
							<span class="text-xs text-slate-500">
								{formatDisplayDate(c.bookingDate)} · {formatAmount(c.amount, c.currency)}
							</span>
						</li>
					{/each}
				</ul>
			</Card>
		{/if}

		{#if review.subscriptions.length > 0}
			<Card title="Prime and Music subscriptions, set aside ({review.subscriptions.length})">
				<p class="mb-2 text-sm text-slate-600">
					These recurring charges are not orders and never appear in the export. They are better
					served by recurring detection and a rule.
				</p>
				<ul class="divide-y divide-slate-100" aria-label="Subscription charges">
					{#each review.subscriptions as c (c.transactionId)}
						<li class="flex flex-wrap items-center justify-between gap-2 py-2 text-sm">
							<span class="text-slate-800">{c.description ?? chargeLabel(c)}</span>
							<span class="text-xs text-slate-500">
								{formatDisplayDate(c.bookingDate)} · {formatAmount(c.amount, c.currency)}
							</span>
						</li>
					{/each}
				</ul>
			</Card>
		{/if}
	{/if}
</div>
