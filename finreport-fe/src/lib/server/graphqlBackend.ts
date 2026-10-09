/**
 * Server-only GraphQL backend access, shared by the `/api/graphql` proxy
 * route, `hooks.server.ts` (per-request `me`) and the `/login` form action —
 * one place that knows how to reach the real backend *or* serve the WP0
 * mocks, so the three callers can't drift (§6, §9 WP5).
 *
 * `PUBLIC_USE_MOCKS=1` is the "never blocked on a running backend" switch
 * (§9 WP5): every GraphQL operation used by the frontend is served from the
 * mock fixtures in `src/lib/graphql/mocks/` plus a tiny in-memory session
 * stand-in for `me`/`login`/`logout`, with no network call at all.
 */
import { env as privateEnv } from '$env/dynamic/private';
import { env as publicEnv } from '$env/dynamic/public';
import type { RequestEvent } from '@sveltejs/kit';
import { amountWithinRange } from '$lib/transactionFilters';
import { compareBySort, DEFAULT_SORT } from '$lib/transactionSort';
import { mockCategoryComparison } from '$lib/graphql/comparisonMock';
import type { TransactionSort } from '$lib/graphql/types';
import {
	applyReimbursementFilter,
	linkFixtureItems,
	mockLinkCandidates,
	mockReimbursementSummary,
	mockSaveLink,
	type LinkableMock
} from '$lib/graphql/linksMock';

import accountsMock from '$lib/graphql/mocks/accounts.json';
import cashflowSummaryMock from '$lib/graphql/mocks/cashflow-summary.json';
import cashflowGraphNetDeficitMock from '$lib/graphql/mocks/cashflow-graph-net-deficit.json';
import cashflowGraphTruncatedMock from '$lib/graphql/mocks/cashflow-graph-truncated.json';
import transactionsMock from '$lib/graphql/mocks/transactions.json';
import categoriesMock from '$lib/graphql/mocks/categories.json';
import categoryBreakdownMock from '$lib/graphql/mocks/category-breakdown.json';
import categoryBreakdownChildrenMock from '$lib/graphql/mocks/category-breakdown-children.json';
import {
	savingsBreakdownMock,
	savingsBreakdownChildrenMock
} from '$lib/graphql/savingsBreakdownMock';
import cashflowGraphCategoryMock from '$lib/graphql/mocks/cashflow-graph-category.json';
import reviewQueueMock from '$lib/graphql/mocks/review-queue.json';
import rulesMock from '$lib/graphql/mocks/rules.json';
import clearTransactionCategoryMock from '$lib/graphql/mocks/clear-transaction-category.json';
import unsplitTransactionMock from '$lib/graphql/mocks/unsplit-transaction.json';
import attentionMock from '$lib/graphql/mocks/attention-summary.json';
import attentionZeroMock from '$lib/graphql/mocks/attention-summary-zero.json';
import tagsMock from '$lib/graphql/mocks/tags.json';
import recurringOverviewMock from '$lib/graphql/mocks/recurring-overview.json';
import goalsMock from '$lib/graphql/mocks/goals.json';
import goalProgressRecurringMock from '$lib/graphql/mocks/goal-progress-recurring.json';
import goalProgressFixedMock from '$lib/graphql/mocks/goal-progress-fixed.json';
import goalTransactionsMock from '$lib/graphql/mocks/goal-transactions.json';
import learningExemptionsMock from '$lib/graphql/mocks/learning-exemptions.json';
import learningExemptionsEmptyMock from '$lib/graphql/mocks/learning-exemptions-empty.json';
import displayAliasesMock from '$lib/graphql/mocks/display-aliases.json';
import displayAliasesEmptyMock from '$lib/graphql/mocks/display-aliases-empty.json';

/** Name of the mock-mode session cookie, mirroring the real `fr_session` cookie's role. */
const MOCK_SESSION_COOKIE = 'fr_session';
const MOCK_USERNAME = 'demo';

export interface GraphqlRequestBody {
	query: string;
	variables?: Record<string, unknown>;
	operationName?: string | null;
}

export interface GraphqlBackendResult {
	status: number;
	/** JSON-serializable GraphQL response body (`{ data }` or `{ errors }`). */
	body: unknown;
	/** Raw `Set-Cookie` header values to relay onto the outer HTTP response. */
	setCookies: string[];
}

export function isMockMode(): boolean {
	return publicEnv.PUBLIC_USE_MOCKS === '1';
}

function backendUrl(): string {
	return privateEnv.GRAPHQL_URL || 'http://localhost:8080/graphql';
}

/** Extract the operation name out of `query CashflowGraph(...) { ... }`. */
function operationNameOf(body: GraphqlRequestBody): string | null {
	if (body.operationName) return body.operationName;
	const match = /\b(?:query|mutation)\s+(\w+)/.exec(body.query);
	return match ? match[1] : null;
}

function invalidCredentialsError() {
	return {
		errors: [
			{
				message: 'Invalid username or password',
				extensions: { code: 'INVALID_CREDENTIALS' }
			}
		]
	};
}

function mockMe(event: RequestEvent): unknown {
	const authed = event.cookies.get(MOCK_SESSION_COOKIE) === 'mock';
	return {
		data: {
			me: authed
				? {
						id: '00000000-0000-0000-0000-000000000001',
						username: MOCK_USERNAME,
						displayName: 'Demo User',
						isAdmin: false
					}
				: null
		}
	};
}

function mockLogin(
	event: RequestEvent,
	variables: Record<string, unknown> | undefined
): GraphqlBackendResult {
	const input = (variables?.input ?? {}) as { username?: string; password?: string };
	if (!input.username || !input.password) {
		return { status: 200, body: invalidCredentialsError(), setCookies: [] };
	}
	// Mirrors the real cookie's shape (§4) closely enough for the proxy/hooks
	// relay path to be exercised end-to-end; the value itself is a fixed
	// sentinel, not a real session token.
	const setCookie = `${MOCK_SESSION_COOKIE}=mock; Path=/; HttpOnly; SameSite=Lax`;
	return {
		status: 200,
		body: {
			data: {
				login: {
					id: '00000000-0000-0000-0000-000000000001',
					username: input.username,
					displayName: 'Demo User',
					isAdmin: false
				}
			}
		},
		setCookies: [setCookie]
	};
}

function mockLogout(): GraphqlBackendResult {
	const setCookie = `${MOCK_SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0`;
	return { status: 200, body: { data: { logout: true } }, setCookies: [setCookie] };
}

interface MockTransaction {
	id: string;
	counterpartyKey?: string | null;
	accountId: string;
	amount: string;
	counterpartyName: string | null;
	/** Fixture-served nickname; search and sort still read `counterpartyName`, as the server does. */
	counterpartyDisplayName?: string | null;
	description: string | null;
	label: {
		status: string;
		category: { slug: string } | null;
		reviewReason?: string | null;
		proposedCategoryPath?: string | null;
	} | null;
	splits: unknown[];
	tags: string[];
	transfer: unknown;
	recurring: { isRecurring: boolean };
	/** Present on the rows `linksFixture.json` adds. */
	link?: unknown;
}

function isSet(v: boolean | null | undefined): v is boolean {
	return v !== undefined && v !== null;
}

/**
 * Applies the `TransactionFilter` fields the filter panel sends over the
 * static fixture in mock mode, so every control is visibly functional
 * without a real backend: `accountIds`, `search` (case-insensitive substring
 * over counterparty + description), `categorySlugs` (OR-ed) and
 * `categorySlugsExact` (the labelled category itself, no descendants), `tags` (AND-ed),
 * `amountMin`/`amountMax` (inclusive bounds on the absolute amount) and the
 * flags `recurring` (the *effective* flag), `transfer`, `needsReview` and
 * `uncategorized` (`true` = no label at all). Dates are not applied: the
 * fixture is one fixed set of rows whatever the period.
 */
function applyInsightFilters(
	items: MockTransaction[],
	filter: Record<string, unknown> | undefined
): MockTransaction[] {
	if (!filter) return items;
	// `reimbursements`: leave out (or keep only) the offsetting side of links.
	items = applyReimbursementFilter(
		items as unknown as LinkableMock[],
		filter.reimbursements as string | undefined
	) as unknown as MockTransaction[];
	const accountIds = filter.accountIds as string[] | undefined;
	const search = (filter.search as string | undefined)?.toLowerCase();
	const categorySlugs = filter.categorySlugs as string[] | undefined;
	const tags = filter.tags as string[] | undefined;
	const amountMin = filter.amountMin as string | undefined;
	const amountMax = filter.amountMax as string | undefined;
	const recurring = filter.recurring as boolean | undefined;
	const transfer = filter.transfer as boolean | undefined;
	const needsReview = filter.needsReview as boolean | undefined;
	const uncategorized = filter.uncategorized as boolean | undefined;
	const counterpartyKeys = filter.counterpartyKeys as string[] | undefined;
	const categorySlugsExact = filter.categorySlugsExact as string[] | undefined;
	return items.filter((item) => {
		if (
			categorySlugsExact?.length &&
			!categorySlugsExact.includes(item.label?.category?.slug ?? '')
		) {
			return false;
		}
		if (counterpartyKeys?.length && !counterpartyKeys.includes(item.counterpartyKey ?? '')) {
			return false;
		}
		if (accountIds?.length && !accountIds.includes(item.accountId)) return false;
		if (search) {
			const haystack = `${item.counterpartyName ?? ''} ${item.description ?? ''}`.toLowerCase();
			if (!haystack.includes(search)) return false;
		}
		if (categorySlugs?.length && !categorySlugs.includes(item.label?.category?.slug ?? '')) {
			return false;
		}
		if (tags && tags.length > 0 && !tags.every((t) => item.tags.includes(t))) return false;
		if (!amountWithinRange(item.amount, amountMin, amountMax)) return false;
		if (isSet(recurring) && item.recurring.isRecurring !== recurring) return false;
		if (isSet(transfer) && Boolean(item.transfer) !== transfer) return false;
		if (isSet(needsReview) && (item.label?.status === 'NEEDS_REVIEW') !== needsReview) return false;
		if (isSet(uncategorized) && (item.label === null) !== uncategorized) return false;
		return true;
	});
}

/** The fixture has no split transactions; give one a split at serve time so
 * the bulk-edit split warning has something to count (fixtures are read-only). */
const MOCK_SPLIT_INDEX = 1;

function mockFixtureItems(): MockTransaction[] {
	const items = [
		...(transactionsMock.data.transactions.items as unknown as MockTransaction[]),
		...(linkFixtureItems() as unknown as MockTransaction[])
	];
	return items.map((item, i) =>
		i === MOCK_SPLIT_INDEX && item.splits.length === 0
			? {
					...item,
					splits: [
						{
							index: 0,
							amount: '-600.00',
							category: findMockCategory('housing') ?? placeholderCategory('housing')
						},
						{
							index: 1,
							amount: '-250.00',
							category: findMockCategory('groceries') ?? placeholderCategory('groceries')
						}
					]
				}
			: item
	);
}

/**
 * `setTransactionsCategory` / `setTransactionsTags`: derive a `BulkEditResult`
 * from the requested filter (`transactionIds` narrows, otherwise the fixture
 * list under the other filters). Nothing is written. To make the partial-failure
 * path reviewable, any edit matching 3 or more rows reports 1 failure; a
 * category edit reports the matched fixture rows that carry splits.
 */
function mockBulkEdit(variables: Record<string, unknown> | undefined, field: string): unknown {
	const filter = variables?.filter as Record<string, unknown> | undefined;
	const ids = filter?.transactionIds as string[] | undefined;
	// A `needsReview` edit targets the held queue, whose rows live in the review fixture.
	const source = filter?.needsReview === true ? mockReviewItems() : mockFixtureItems();
	let matchedItems = applyInsightFilters(source, filter);
	if (ids) matchedItems = matchedItems.filter((item) => ids.includes(item.id));
	const matched = ids ? ids.length : matchedItems.length;
	const failed = matched >= 3 ? 1 : 0;
	const splitsCleared =
		field === 'setTransactionsCategory'
			? matchedItems.filter((item) => item.splits.length > 0).length
			: 0;
	return { [field]: { matched, applied: matched - failed, failed, splitsCleared } };
}

/** The review-queue fixture shaped like full transactions, so the shared filters apply to it. */
function mockReviewItems(): MockTransaction[] {
	return reviewQueueMock.data.reviewQueue.transactions.map((item) => ({
		splits: [],
		tags: [],
		transfer: null,
		recurring: { isRecurring: false },
		...item
	})) as unknown as MockTransaction[];
}

/** `ReviewQueue`: the held fixture, paged like the real query; the queue lists every held row. */
function mockReviewQueue(variables: Record<string, unknown> | undefined): unknown {
	const page = variables?.page as { limit?: number; offset?: number } | undefined;
	const offset = page?.offset ?? 0;
	const limit = page?.limit ?? Number.MAX_SAFE_INTEGER;
	const all = reviewQueueMock.data.reviewQueue;
	return {
		reviewQueue: { ...all, transactions: mockReviewItems().slice(offset, offset + limit) }
	};
}

/**
 * `HeldMerchantGroups`: derived from the held fixture the way the backend
 * aggregates it, so the groups always agree with `ReviewHeldTransactions`
 * and the bulk-edit mock: signed net total, most common name/proposal
 * (ties alphabetical), distinct reasons, the keyless rows as one bucket,
 * largest group first.
 */
function mockHeldGroups(variables: Record<string, unknown> | undefined): unknown {
	const page = variables?.page as { limit?: number; offset?: number } | undefined;
	const rows = mockReviewItems();
	const byKey = new Map<string | null, MockTransaction[]>();
	for (const row of rows) {
		const key = row.counterpartyKey || null;
		byKey.set(key, [...(byKey.get(key) ?? []), row]);
	}
	const mode = (values: (string | null | undefined)[]): { value: string | null; count: number } => {
		const counts = new Map<string, number>();
		for (const v of values) if (v) counts.set(v, (counts.get(v) ?? 0) + 1);
		const best = [...counts.entries()].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))[0];
		return best ? { value: best[0], count: best[1] } : { value: null, count: 0 };
	};
	const groups = [...byKey.entries()].map(([key, items]) => {
		const proposed = mode(items.map((i) => i.label?.proposedCategoryPath));
		const total = items.reduce((sum, i) => sum + Math.round(Number(i.amount) * 100), 0);
		const reasons = [...new Set(items.map((i) => i.label?.reviewReason).filter(Boolean))];
		return {
			counterpartyKey: key,
			displayName:
				key === null
					? 'No merchant key'
					: (mode(items.map((i) => i.counterpartyName)).value ?? key),
			heldCount: items.length,
			totalAmount: (total / 100).toFixed(2),
			currency: 'EUR',
			reviewReasons: reasons,
			proposedCategoryPath: proposed.value,
			proposedCategoryVotes: proposed.count
		};
	});
	groups.sort((a, b) => b.heldCount - a.heldCount || a.displayName.localeCompare(b.displayName));
	const offset = page?.offset ?? 0;
	return {
		heldMerchantGroups: {
			groups: groups.slice(offset, offset + (page?.limit ?? groups.length)),
			groupCount: groups.length,
			heldCount: rows.length
		}
	};
}

/** `ReviewHeldTransactions`: held rows narrowed by the transaction filter (e.g. `counterpartyKeys`). */
function mockReviewHeld(variables: Record<string, unknown> | undefined): unknown {
	const page = variables?.page as { limit?: number; offset?: number } | undefined;
	const filtered = applyInsightFilters(
		mockReviewItems(),
		variables?.filter as Record<string, unknown> | undefined
	);
	const offset = page?.offset ?? 0;
	return {
		transactions: {
			items: filtered.slice(offset, offset + (page?.limit ?? filtered.length)),
			totalCount: filtered.length
		}
	};
}

function mockTransactions(variables: Record<string, unknown> | undefined): unknown {
	const items = mockFixtureItems();
	const filtered = applyInsightFilters(items, variables?.filter as Record<string, unknown>);
	// Sort the whole filtered set before paging, as the server does.
	const order = compareBySort((variables?.sort as TransactionSort | undefined) ?? DEFAULT_SORT);
	filtered.sort(order as unknown as (a: MockTransaction, b: MockTransaction) => number);
	const page = (variables?.page ?? {}) as { limit?: number; offset?: number };
	const limit = page.limit ?? transactionsMock.data.transactions.limit;
	const offset = page.offset ?? 0;
	return {
		transactions: {
			...transactionsMock.data.transactions,
			items: filtered.slice(offset, offset + limit),
			totalCount: filtered.length,
			limit,
			offset
		}
	};
}

/**
 * Pick a cashflow-graph fixture: the `CATEGORY` dimension (§5/§6 WP5) gets
 * its own fixture regardless of account scoping, otherwise the richer
 * net/deficit fixture unless the request is scoped to exactly one account.
 */
function mockCashflowGraph(variables: Record<string, unknown> | undefined): unknown {
	const dimensions = (variables?.grouping as { dimensions?: unknown[] } | undefined)?.dimensions;
	if (Array.isArray(dimensions) && dimensions.includes('CATEGORY')) {
		return cashflowGraphCategoryMock.data;
	}
	const accountIds = (variables?.filter as { accountIds?: unknown[] } | undefined)?.accountIds;
	if (Array.isArray(accountIds) && accountIds.length === 1) {
		return cashflowGraphTruncatedMock.data;
	}
	return cashflowGraphNetDeficitMock.data;
}

interface MockCategory {
	id: string;
	slug: string;
	name: string;
	kind: string;
	parentId: string | null;
	depth: number;
	archived: boolean;
	origin: string;
}

/**
 * `/admin/rules`, `/admin/categories` and `/admin/review` (§5, §10 WP6)
 * mutations: the real backend only publishes-then-upserts (§2.1, async via
 * the labeler), so a static fixture can't stand in for the round-trip the
 * way `ReviewQueue`'s does. These echo the caller's own input back as the
 * "fresh row", which is enough for the admin/review screens to work
 * against in mock mode.
 */
function findMockCategory(slug: unknown): MockCategory | undefined {
	return (
		(categoriesMock.data.categories as MockCategory[]).find((c) => c.slug === slug) ??
		mockEnsuredCategories.get(slug as string)
	);
}

/** Categories made through `ensureCategory` in mock mode, so a later pick of one resolves. */
const mockEnsuredCategories = new Map<string, MockCategory>();

/** Placeholder used when a mutation is given a slug the fixture doesn't know about. */
function placeholderCategory(slug: string | undefined): MockCategory {
	return {
		id: '00000000-0000-0000-0000-000000000000',
		slug: slug ?? '',
		name: slug ?? '',
		kind: 'EXPENSE',
		parentId: null,
		depth: 1,
		archived: false,
		origin: 'user'
	};
}

/** `setTransactionCategory`/`splitTransaction`: echo back the requested
 * transaction/category from `variables` instead of the fixture's fixed
 * category, so picking a different category in the UI is visible in mock
 * mode rather than always answering "Groceries". */
function mockSetTransactionCategory(variables: Record<string, unknown> | undefined): unknown {
	const transactionId = (variables?.transactionId as string) ?? '';
	const category = findMockCategory(variables?.categorySlug);
	return {
		setTransactionCategory: {
			id: transactionId,
			label: {
				category: category
					? { id: category.id, slug: category.slug, name: category.name, kind: category.kind }
					: null,
				source: 'USER',
				status: 'RESOLVED'
			},
			// The real mutation clears splits; echo that so the UI can show it.
			splits: []
		}
	};
}

/** `setTransactionTags`/`setTransactionRecurring` (iteration 3 §4): echo
 * back the requested transaction with the mutation's own arguments applied,
 * same "visible in mock mode" rationale as `mockSetTransactionCategory`. */
function mockSetTransactionTags(variables: Record<string, unknown> | undefined): unknown {
	const transactionId = (variables?.transactionId as string) ?? '';
	const tags = (variables?.tags as string[] | undefined) ?? [];
	return { setTransactionTags: { id: transactionId, tags: [...tags].sort() } };
}

/** `setTransactionNote`: echo the trimmed note, blank/null clearing it, as the backend does. */
function mockSetTransactionNote(variables: Record<string, unknown> | undefined): unknown {
	const transactionId = (variables?.transactionId as string) ?? '';
	const raw = (variables?.note as string | null | undefined) ?? '';
	return { setTransactionNote: { id: transactionId, note: raw.trim() === '' ? null : raw.trim() } };
}

function mockSetTransactionRecurring(variables: Record<string, unknown> | undefined): unknown {
	const transactionId = (variables?.transactionId as string) ?? '';
	const recurring = (variables?.recurring as boolean | null | undefined) ?? null;
	return {
		setTransactionRecurring: {
			id: transactionId,
			recurring: {
				isRecurring: recurring ?? false,
				source: recurring === null ? 'AUTO' : 'USER',
				seriesId: null,
				cadence: null,
				medianAmount: null
			}
		}
	};
}

function mockSplitTransaction(variables: Record<string, unknown> | undefined): unknown {
	const transactionId = (variables?.transactionId as string) ?? '';
	const parts = (variables?.parts as { amount: string; categorySlug: string }[] | undefined) ?? [];
	return {
		splitTransaction: {
			id: transactionId,
			splits: parts.map((part, index) => {
				const category = findMockCategory(part.categorySlug);
				return {
					index,
					amount: part.amount,
					category: category
						? { id: category.id, slug: category.slug, name: category.name }
						: { id: '', slug: part.categorySlug, name: part.categorySlug }
				};
			})
		}
	};
}

function mockCreateOrUpdateRule(
	variables: Record<string, unknown> | undefined,
	existingId?: string
): unknown {
	const input = (variables?.input ?? {}) as {
		name?: string;
		categorySlug?: string;
		conditions?: unknown;
		priority?: number;
	};
	const existing = existingId ? rulesMock.data.rules.find((r) => r.id === existingId) : undefined;
	const category =
		findMockCategory(input.categorySlug) ??
		existing?.category ??
		placeholderCategory(input.categorySlug);
	return {
		id: existingId ?? crypto.randomUUID(),
		name: input.name ?? existing?.name ?? 'Untitled rule',
		category: { id: category.id, slug: category.slug, name: category.name, kind: category.kind },
		conditions: input.conditions ?? existing?.conditions ?? {},
		priority: input.priority ?? existing?.priority ?? 0,
		state: existing?.state ?? 'ACTIVE',
		origin: existing?.origin ?? 'USER',
		autoApproved: existing?.autoApproved ?? false,
		confidence: existing?.confidence ?? null,
		evidenceCount: existing?.evidenceCount ?? 0,
		matchingTransactionCount: existing?.matchingTransactionCount ?? 0,
		createdAt: existing?.createdAt ?? new Date().toISOString()
	};
}

function mockSetRuleState(variables: Record<string, unknown> | undefined): unknown {
	const id = variables?.id as string | undefined;
	const state = variables?.state as string | undefined;
	const existing = rulesMock.data.rules.find((r) => r.id === id);
	return {
		...((existing ?? mockCreateOrUpdateRule(undefined, id)) as Record<string, unknown>),
		id,
		state: state ?? existing?.state ?? 'ACTIVE'
	};
}

interface MockLearningExemption {
	counterpartyKey: string;
	displayName: string;
	transactionCount: number;
	exemptedAt: string;
}

/**
 * The exempt-merchant list is the one piece of mock state that is not a
 * fixed fixture: exempting and undoing must be visibly working end to end
 * (including the empty state), so it is kept in memory for the lifetime of
 * the dev server. Seeded from the fixture; a restart resets it.
 */
const mockExemptions: MockLearningExemption[] = [
	...(learningExemptionsMock.data.learningExemptions as MockLearningExemption[])
];

function mockLearningExemptions(): unknown {
	return mockExemptions.length === 0
		? learningExemptionsEmptyMock.data
		: { learningExemptions: [...mockExemptions] };
}

function mockExemptFromLearning(variables: Record<string, unknown> | undefined): unknown {
	const key = String(variables?.counterpartyKey ?? '')
		.trim()
		.toLowerCase();
	const existing = mockExemptions.find((e) => e.counterpartyKey === key);
	if (existing) return existing;
	// Name and count come from the rule fixture where the merchant has one.
	const rule = rulesMock.data.rules.find(
		(r) => (r.conditions as { counterparty_key?: string }).counterparty_key === key
	);
	const created: MockLearningExemption = {
		counterpartyKey: key,
		displayName: rule ? rule.name.split(' → ')[0] : key,
		transactionCount: rule?.matchingTransactionCount ?? 0,
		exemptedAt: new Date().toISOString()
	};
	mockExemptions.unshift(created);
	return created;
}

function mockRemoveLearningExemption(variables: Record<string, unknown> | undefined): boolean {
	const key = String(variables?.counterpartyKey ?? '')
		.trim()
		.toLowerCase();
	const index = mockExemptions.findIndex((e) => e.counterpartyKey === key);
	if (index >= 0) mockExemptions.splice(index, 1);
	return index >= 0;
}

interface MockDisplayAlias {
	kind: string;
	key: string;
	alias: string;
	rawName: string;
	transactionCount: number | null;
	updatedAt: string;
}

/**
 * Display aliases are in-memory mock state, like the exemptions: setting,
 * editing and removing must visibly work end to end, including the empty
 * state. Seeded from the fixture; a restart resets it.
 */
const mockAliases: MockDisplayAlias[] = [
	...(displayAliasesMock.data.displayAliases as MockDisplayAlias[])
];

function mockDisplayAliases(): unknown {
	return mockAliases.length === 0
		? displayAliasesEmptyMock.data
		: { displayAliases: [...mockAliases] };
}

function mockSetDisplayAlias(variables: Record<string, unknown> | undefined): MockDisplayAlias {
	const kind = String(variables?.kind ?? 'COUNTERPARTY');
	const rawKey = String(variables?.key ?? '').trim();
	const key = kind === 'ACCOUNT' ? rawKey : rawKey.toLowerCase();
	const account = accountsMock.data.accounts.find((a) => a.id === key);
	const entry: MockDisplayAlias = {
		kind,
		key,
		alias: String(variables?.alias ?? '').trim(),
		rawName: kind === 'ACCOUNT' ? (account?.label ?? key) : rawKey,
		transactionCount: kind === 'ACCOUNT' ? null : 0,
		updatedAt: new Date().toISOString()
	};
	const index = mockAliases.findIndex((a) => a.kind === kind && a.key === key);
	if (index >= 0) {
		entry.rawName = mockAliases[index].rawName;
		entry.transactionCount = mockAliases[index].transactionCount;
		mockAliases.splice(index, 1);
	}
	mockAliases.unshift(entry);
	return entry;
}

function mockRemoveDisplayAlias(variables: Record<string, unknown> | undefined): boolean {
	const kind = String(variables?.kind ?? '');
	const rawKey = String(variables?.key ?? '').trim();
	const key = kind === 'ACCOUNT' ? rawKey : rawKey.toLowerCase();
	const index = mockAliases.findIndex((a) => a.kind === kind && a.key === key);
	if (index >= 0) mockAliases.splice(index, 1);
	return index >= 0;
}

/** `reapplyRule` returns how many transactions it will re-queue (§5); a fixed, plausible count in mock mode. */
function mockReapplyRule(): unknown {
	return 7;
}

function mockCreateOrUpdateCategory(
	variables: Record<string, unknown> | undefined,
	existingId?: string
): unknown {
	const input = (variables?.input ?? {}) as {
		slug?: string;
		name?: string;
		kind?: string;
		parentSlug?: string | null;
	};
	const parent = input.parentSlug ? findMockCategory(input.parentSlug) : undefined;
	return {
		id: existingId ?? crypto.randomUUID(),
		slug: input.slug ?? '',
		name: input.name ?? '',
		kind: input.kind ?? 'EXPENSE',
		parentId: parent?.id ?? null,
		depth: parent ? parent.depth + 1 : 1,
		archived: false,
		origin: 'user'
	};
}

function mockEnsureCategory(variables: Record<string, unknown> | undefined): unknown {
	const made = mockCreateOrUpdateCategory(variables) as MockCategory;
	const existing = findMockCategory(made.slug);
	if (existing) return existing;
	mockEnsuredCategories.set(made.slug, made);
	return made;
}

function mockRenameCategory(variables: Record<string, unknown> | undefined): unknown {
	const id = variables?.id as string | undefined;
	const name = variables?.name as string | undefined;
	const existing = categoriesMock.data.categories.find((c) => c.id === id);
	return { id, slug: existing?.slug ?? '', name: name ?? existing?.name ?? '' };
}

function mockArchiveCategory(variables: Record<string, unknown> | undefined): unknown {
	const id = variables?.id as string | undefined;
	const existing = categoriesMock.data.categories.find((c) => c.id === id);
	return { id, slug: existing?.slug ?? '', archived: true };
}

// ---------------------------------------------------------------------------
// Goals (iteration 4 §4): the real resolvers are NOT_IMPLEMENTED until WP-B,
// so WP-C builds against these.
// ---------------------------------------------------------------------------

interface MockGoal {
	id: string;
	scope: { categories: MockCategory[]; tags: string[]; combine: string; tagCombine: string };
	[key: string]: unknown;
}

function findMockGoal(id: unknown): MockGoal | undefined {
	return (goalsMock.data.goals as unknown as MockGoal[]).find((g) => g.id === id);
}

/** The fixed-range `saving_target` fixture for its own id, the recurring one otherwise. */
function mockGoalProgress(variables: Record<string, unknown> | undefined): unknown {
	const fixedId = goalProgressFixedMock.data.goalProgress.goal.id;
	return variables?.id === fixedId ? goalProgressFixedMock.data : goalProgressRecurringMock.data;
}

/**
 * `goalTransactions`: the goal's own fixture rows, narrowed to the requested
 * `[startDate, endDate]` (inclusive) and paged by `PageInput`, so a period
 * click on the goal page visibly shrinks the list. `totalCount` is the count
 * before paging, as the real resolver reports it.
 */
function mockGoalTransactions(variables: Record<string, unknown> | undefined): unknown {
	const rowsByGoal = goalTransactionsMock.rowsByGoal as Record<string, { bookingDate: string }[]>;
	const start = String(variables?.startDate ?? '');
	const end = String(variables?.endDate ?? '');
	const inRange = (
		rowsByGoal[String(variables?.id)] ?? goalTransactionsMock.data.goalTransactions.items
	).filter((row) => (!start || row.bookingDate >= start) && (!end || row.bookingDate <= end));
	// Unsorted keeps the fixture's own order, as the real resolver keeps the engine's.
	const sort = variables?.sort as TransactionSort | undefined;
	const sorted = sort
		? [...inRange].sort(compareBySort(sort) as (a: unknown, b: unknown) => number)
		: inRange;
	const page = (variables?.page ?? {}) as { limit?: number; offset?: number };
	const limit = page.limit ?? 50;
	const offset = page.offset ?? 0;
	return {
		goalTransactions: {
			items: sorted.slice(offset, offset + limit),
			totalCount: inRange.length,
			limit,
			offset
		}
	};
}

/** `createGoal`/`updateGoal`: echo the caller's `GoalInput` back as the stored goal. */
function mockSaveGoal(
	variables: Record<string, unknown> | undefined,
	existingId?: string
): unknown {
	const input = (variables?.input ?? {}) as Record<string, unknown>;
	const slugs = (input.categorySlugs as string[] | undefined) ?? [];
	return {
		id: existingId ?? crypto.randomUUID(),
		name: input.name ?? '',
		type: input.type ?? 'SPENDING_LIMIT',
		amount: input.amount ?? '0.0000',
		currency: input.currency ?? 'EUR',
		scope: {
			categories: slugs.map((slug) => findMockCategory(slug) ?? placeholderCategory(slug)),
			tags: (input.tags as string[] | undefined) ?? [],
			combine: input.combine ?? 'ALL',
			tagCombine: input.tagCombine ?? 'ALL'
		},
		periodKind: input.periodKind ?? 'RECURRING',
		cadence: input.cadence ?? null,
		startDate: input.startDate ?? null,
		endDate: input.endDate ?? null,
		archived: false
	};
}

function mockArchiveGoal(variables: Record<string, unknown> | undefined): unknown {
	const existing = findMockGoal(variables?.id) ?? goalsMock.data.goals[0];
	return { ...existing, id: variables?.id ?? existing.id, archived: true };
}

/**
 * A breakdown scoped to one category at a deeper level (an expanded dashboard
 * row) gets that category's child fixture; anything else gets the top level.
 * With several scoped slugs (a panel filter overlapping the parent) the
 * shortest one present in the fixture is the parent.
 */
function mockCategoryBreakdown(variables: Record<string, unknown> | undefined): unknown {
	const filter = variables?.filter as { categorySlugs?: string[] } | undefined;
	const level = Number(variables?.level ?? 1);
	// The dashboard asks this same operation twice per load, once per card, so
	// the kind - not the operation name - is what tells the two apart here.
	const saving = variables?.kind === 'SAVING';
	if (level > 1 && filter?.categorySlugs?.length) {
		const children = (
			saving ? savingsBreakdownChildrenMock : categoryBreakdownChildrenMock.data
		) as Record<string, unknown>;
		const parent = [...filter.categorySlugs]
			.sort((a, b) => a.length - b.length)
			.find((slug) => slug in children);
		if (parent) return { categoryBreakdown: children[parent] };
		return {
			categoryBreakdown: { rows: [], uncategorized: null, needsReview: null, currency: 'EUR' }
		};
	}
	return saving ? savingsBreakdownMock : categoryBreakdownMock.data;
}

function mockResponse(event: RequestEvent, body: GraphqlRequestBody): GraphqlBackendResult | null {
	switch (operationNameOf(body)) {
		case 'Me':
			return { status: 200, body: mockMe(event), setCookies: [] };
		case 'Login':
			return mockLogin(event, body.variables);
		case 'Logout':
			return mockLogout();
		case 'Accounts':
			return { status: 200, body: { data: accountsMock.data }, setCookies: [] };
		case 'Transactions':
			return { status: 200, body: { data: mockTransactions(body.variables) }, setCookies: [] };
		case 'CashflowSummary':
			return { status: 200, body: { data: cashflowSummaryMock.data }, setCookies: [] };
		case 'CashflowGraph':
			return { status: 200, body: { data: mockCashflowGraph(body.variables) }, setCookies: [] };
		// Iteration-2 (§5) stub operations: the WP0 mocks exist so WP5/WP6 can
		// build against a stable shape; resolvers on the real backend still
		// return NOT_IMPLEMENTED until their owning WP lands.
		case 'CategoryBreakdown':
			return { status: 200, body: { data: mockCategoryBreakdown(body.variables) }, setCookies: [] };
		case 'CategoryComparison':
			return {
				status: 200,
				body: { data: mockCategoryComparison(body.variables) },
				setCookies: []
			};
		case 'Categories':
			return { status: 200, body: { data: categoriesMock.data }, setCookies: [] };
		case 'ReviewQueue':
			return {
				status: 200,
				body: { data: mockReviewQueue(body.variables) },
				setCookies: []
			};
		case 'HeldMerchantGroups':
			return { status: 200, body: { data: mockHeldGroups(body.variables) }, setCookies: [] };
		case 'ReviewHeldTransactions':
			return { status: 200, body: { data: mockReviewHeld(body.variables) }, setCookies: [] };
		case 'Rules':
			return { status: 200, body: { data: rulesMock.data }, setCookies: [] };
		case 'RecentlyAutoApprovedRules':
			// Field name differs from `Rules` (`recentlyAutoApprovedRules`, not
			// `rules`), and only auto-approved learned rules qualify (§2.8).
			return {
				status: 200,
				body: {
					data: {
						recentlyAutoApprovedRules: rulesMock.data.rules.filter(
							(r) => r.origin === 'LEARNED' && r.autoApproved
						)
					}
				},
				setCookies: []
			};
		// Iteration-3 (§4) stub operations: same "WP0 mock, real backend still
		// NOT_IMPLEMENTED" shape as the iteration-2 block above.
		// MOCK_ATTENTION=zero serves the nothing-to-do state.
		case 'AttentionSummary':
			return {
				status: 200,
				body: {
					data: (privateEnv.MOCK_ATTENTION === 'zero' ? attentionZeroMock : attentionMock).data
				},
				setCookies: []
			};
		case 'Tags':
			return { status: 200, body: { data: tagsMock.data }, setCookies: [] };
		case 'RecurringSeries':
			return { status: 200, body: { data: recurringOverviewMock.data }, setCookies: [] };
		// Iteration-4 (§4) goal operations, same "WP0 mock" shape as above.
		case 'Goals':
			return { status: 200, body: { data: goalsMock.data }, setCookies: [] };
		case 'Goal':
			return {
				status: 200,
				body: { data: { goal: findMockGoal(body.variables?.id) ?? null } },
				setCookies: []
			};
		case 'GoalProgress':
			return { status: 200, body: { data: mockGoalProgress(body.variables) }, setCookies: [] };
		case 'GoalTransactions':
			return { status: 200, body: { data: mockGoalTransactions(body.variables) }, setCookies: [] };
		case 'CreateGoal':
			return {
				status: 200,
				body: { data: { createGoal: mockSaveGoal(body.variables) } },
				setCookies: []
			};
		case 'UpdateGoal':
			return {
				status: 200,
				body: { data: { updateGoal: mockSaveGoal(body.variables, body.variables?.id as string) } },
				setCookies: []
			};
		case 'ArchiveGoal':
			return {
				status: 200,
				body: { data: { archiveGoal: mockArchiveGoal(body.variables) } },
				setCookies: []
			};
		case 'CreateTransactionLink':
			return {
				status: 200,
				body: {
					data: {
						createTransactionLink: mockSaveLink(
							body.variables,
							mockFixtureItems() as unknown as LinkableMock[]
						)
					}
				},
				setCookies: []
			};
		case 'UpdateTransactionLink':
			return {
				status: 200,
				body: {
					data: {
						updateTransactionLink: mockSaveLink(
							body.variables,
							mockFixtureItems() as unknown as LinkableMock[],
							body.variables?.id as string | undefined
						)
					}
				},
				setCookies: []
			};
		case 'RemoveTransactionLink':
			return { status: 200, body: { data: { removeTransactionLink: true } }, setCookies: [] };
		case 'LinkCandidates':
			return {
				status: 200,
				body: {
					data: mockLinkCandidates(body.variables, mockFixtureItems() as unknown as LinkableMock[])
				},
				setCookies: []
			};
		case 'ReimbursementSummary':
			return {
				status: 200,
				body: {
					data: mockReimbursementSummary(
						applyInsightFilters(
							mockFixtureItems(),
							body.variables?.filter as Record<string, unknown> | undefined
						) as unknown as LinkableMock[]
					)
				},
				setCookies: []
			};
		case 'SetTransactionTags':
			return {
				status: 200,
				body: { data: mockSetTransactionTags(body.variables) },
				setCookies: []
			};
		case 'SetTransactionsCategory':
			return {
				status: 200,
				body: { data: mockBulkEdit(body.variables, 'setTransactionsCategory') },
				setCookies: []
			};
		case 'SetTransactionsTags':
			return {
				status: 200,
				body: { data: mockBulkEdit(body.variables, 'setTransactionsTags') },
				setCookies: []
			};
		case 'SetTransactionNote':
			return {
				status: 200,
				body: { data: mockSetTransactionNote(body.variables) },
				setCookies: []
			};
		case 'SetTransactionRecurring':
			return {
				status: 200,
				body: { data: mockSetTransactionRecurring(body.variables) },
				setCookies: []
			};
		case 'SetTransactionCategory':
			return {
				status: 200,
				body: { data: mockSetTransactionCategory(body.variables) },
				setCookies: []
			};
		case 'ClearTransactionCategory':
			return { status: 200, body: { data: clearTransactionCategoryMock.data }, setCookies: [] };
		case 'SplitTransaction':
			return { status: 200, body: { data: mockSplitTransaction(body.variables) }, setCookies: [] };
		case 'UnsplitTransaction':
			return { status: 200, body: { data: unsplitTransactionMock.data }, setCookies: [] };
		case 'CreateRule':
			return {
				status: 200,
				body: { data: { createRule: mockCreateOrUpdateRule(body.variables) } },
				setCookies: []
			};
		case 'UpdateRule':
			return {
				status: 200,
				body: {
					data: { updateRule: mockCreateOrUpdateRule(body.variables, body.variables?.id as string) }
				},
				setCookies: []
			};
		case 'SetRuleState':
			return {
				status: 200,
				body: { data: { setRuleState: mockSetRuleState(body.variables) } },
				setCookies: []
			};
		case 'LearningExemptions':
			return {
				status: 200,
				body: { data: mockLearningExemptions() },
				setCookies: []
			};
		case 'ExemptFromLearning':
			return {
				status: 200,
				body: { data: { exemptFromLearning: mockExemptFromLearning(body.variables) } },
				setCookies: []
			};
		case 'RemoveLearningExemption':
			return {
				status: 200,
				body: { data: { removeLearningExemption: mockRemoveLearningExemption(body.variables) } },
				setCookies: []
			};
		case 'DisplayAliases':
			return { status: 200, body: { data: mockDisplayAliases() }, setCookies: [] };
		case 'SetDisplayAlias':
			return {
				status: 200,
				body: { data: { setDisplayAlias: mockSetDisplayAlias(body.variables) } },
				setCookies: []
			};
		case 'RemoveDisplayAlias':
			return {
				status: 200,
				body: { data: { removeDisplayAlias: mockRemoveDisplayAlias(body.variables) } },
				setCookies: []
			};
		case 'ReapplyRule':
			return { status: 200, body: { data: { reapplyRule: mockReapplyRule() } }, setCookies: [] };
		case 'CreateCategory':
			return {
				status: 200,
				body: { data: { createCategory: mockCreateOrUpdateCategory(body.variables) } },
				setCookies: []
			};
		// `ensureCategory` is create-or-return-existing, so a mock that always
		// echoes the requested category is faithful: the real mutation only
		// differs from `createCategory` by not failing on a repeat.
		case 'EnsureCategory':
			return {
				status: 200,
				body: { data: { ensureCategory: mockEnsureCategory(body.variables) } },
				setCookies: []
			};
		case 'RenameCategory':
			return {
				status: 200,
				body: { data: { renameCategory: mockRenameCategory(body.variables) } },
				setCookies: []
			};
		case 'ArchiveCategory':
			return {
				status: 200,
				body: { data: { archiveCategory: mockArchiveCategory(body.variables) } },
				setCookies: []
			};
		default:
			return null;
	}
}

/**
 * Apply raw `Set-Cookie` strings from a `forwardGraphql` result onto the
 * current request's response via SvelteKit's `cookies` API (used by the
 * `/login` and `/logout` actions, which can't just copy the header the way
 * the `/api/graphql` route does — form actions don't expose a raw
 * `Response`). Parsing is intentionally minimal: the backend is the only
 * producer of these cookies and its shape is fixed by §4.
 */
export function relaySetCookies(event: RequestEvent, setCookies: string[]): void {
	for (const raw of setCookies) {
		const [pair, ...attrParts] = raw.split(';').map((part) => part.trim());
		const eq = pair.indexOf('=');
		if (eq === -1) continue;
		const name = pair.slice(0, eq);
		const value = pair.slice(eq + 1);
		const attrs: Record<string, string | boolean> = {};
		for (const attr of attrParts) {
			const [key, val] = attr.split('=').map((s) => s.trim());
			attrs[key.toLowerCase()] = val ?? true;
		}
		const maxAge = attrs['max-age'] ? Number(attrs['max-age']) : undefined;
		if (maxAge === 0) {
			event.cookies.delete(name, { path: (attrs['path'] as string) || '/' });
			continue;
		}
		event.cookies.set(name, value, {
			path: (attrs['path'] as string) || '/',
			httpOnly: Boolean(attrs['httponly']),
			secure: Boolean(attrs['secure']),
			sameSite: (attrs['samesite'] as 'lax' | 'strict' | 'none' | undefined)?.toLowerCase() as
				| 'lax'
				| 'strict'
				| 'none'
				| undefined,
			maxAge
		});
	}
}

function getSetCookies(headers: Headers): string[] {
	// `Headers.getSetCookie()` is the only correct way to read multiple
	// `Set-Cookie` values (`.get()` would merge them with commas, which is
	// not valid for cookies); fall back for runtimes without it.
	if (typeof headers.getSetCookie === 'function') return headers.getSetCookie();
	const single = headers.get('set-cookie');
	return single ? [single] : [];
}

/**
 * Forward a parsed GraphQL request to the real backend, or synthesize a
 * mock-mode response. Uses the event's own `fetch` so cookies/credentials
 * behave consistently with the rest of SvelteKit's server-side fetch
 * tracking.
 */
export async function forwardGraphql(
	event: RequestEvent,
	body: GraphqlRequestBody
): Promise<GraphqlBackendResult> {
	if (isMockMode()) {
		const mocked = mockResponse(event, body);
		if (mocked) return mocked;
		// An operation the mocks don't model (shouldn't happen for this
		// frontend's own queries) — fail loudly rather than guessing.
		return {
			status: 200,
			body: { errors: [{ message: `No mock for operation in: ${body.query.slice(0, 80)}` }] },
			setCookies: []
		};
	}

	const headers: Record<string, string> = { 'content-type': 'application/json' };
	const cookie = event.request.headers.get('cookie');
	if (cookie) headers['cookie'] = cookie;
	const origin = event.request.headers.get('origin');
	if (origin) headers['origin'] = origin;

	const upstream = await event.fetch(backendUrl(), {
		method: 'POST',
		headers,
		body: JSON.stringify(body)
	});
	const json = await upstream.json();
	return { status: upstream.status, body: json, setCookies: getSetCookies(upstream.headers) };
}
