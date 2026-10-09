import { createGraphqlClient } from '$lib/graphqlClient';
import {
	ACCOUNTS_QUERY,
	ATTENTION_SUMMARY_QUERY,
	CASHFLOW_GRAPH_QUERY,
	CASHFLOW_SUMMARY_QUERY,
	CATEGORIES_QUERY,
	CATEGORY_BREAKDOWN_QUERY,
	REIMBURSEMENT_SUMMARY_QUERY,
	TAGS_QUERY,
	TRANSACTIONS_QUERY
} from '$lib/graphql/queries';
import type {
	Account,
	AttentionSummary,
	CashflowDimension,
	CashflowGraph,
	CashflowSummary,
	Category,
	CategoryBreakdown,
	ReimbursementSummary,
	TagCount,
	TransactionFilter,
	TransactionPage
} from '$lib/graphql/types';
import {
	defaultGranularity,
	parsePeriodSelection,
	rangeFromParams,
	toDateInputValue,
	type Granularity
} from '$lib/period';
import { layerSelection, parsePanelFilters, toTransactionFilter } from '$lib/transactionFilters';
import { parseSort, sortVariable } from '$lib/transactionSort';
import type { PageLoad } from './$types';

const PAGE_LIMIT = 50;

/**
 * The chart selection (§6): a Sankey/breakdown click narrows only the
 * transaction list, on top of the filter panel. It has its own `sel*` params
 * so it never fights the panel, which owns `accountIds`, `categorySlugs`, ...
 */
function readDrilldown(url: URL) {
	const accountIds = url.searchParams.get('selAccountIds')?.split(',').filter(Boolean);
	const counterpartyNames = url.searchParams.get('selCounterparties')?.split(',').filter(Boolean);
	const hasCounterpartyParam = url.searchParams.get('selHasCounterparty');
	const categorySlugs = url.searchParams.get('selCategorySlugs')?.split(',').filter(Boolean);
	const categorySlugsExact = url.searchParams
		.get('selCategorySlugsExact')
		?.split(',')
		.filter(Boolean);
	const uncategorizedParam = url.searchParams.get('selUncategorized');
	const needsReviewParam = url.searchParams.get('selNeedsReview');
	return {
		accountIds: accountIds?.length ? accountIds : undefined,
		counterpartyNames: counterpartyNames?.length ? counterpartyNames : undefined,
		hasCounterparty: hasCounterpartyParam === 'false' ? false : undefined,
		categorySlugs: categorySlugs?.length ? categorySlugs : undefined,
		// The category itself, not its descendants (a breakdown's "(no subcategory)" row).
		categorySlugsExact: categorySlugsExact?.length ? categorySlugsExact : undefined,
		uncategorized: uncategorizedParam === 'true' ? true : undefined,
		// Tri-state: an "Uncategorized" bar click sends `false` (held labels are
		// a separate bar), a "Needs review" bar click sends `true`.
		needsReview:
			needsReviewParam === 'true' ? true : needsReviewParam === 'false' ? false : undefined
	};
}

/** Sankey dimension toggle (§6): `counterparty` (iteration 1 default) or `category`. */
function readSankeyDimension(url: URL): 'counterparty' | 'category' {
	return url.searchParams.get('sankey') === 'category' ? 'category' : 'counterparty';
}

function groupingForDimension(dimension: 'counterparty' | 'category'): {
	dimensions: CashflowDimension[];
} | null {
	if (dimension === 'category') return { dimensions: ['INCOME_SOURCE', 'ACCOUNT', 'CATEGORY'] };
	return null;
}

export const load: PageLoad = async ({ fetch, url }) => {
	const today = new Date();
	const preset = parsePeriodSelection(url.searchParams.get('preset'));
	const range = rangeFromParams(url.searchParams, preset, today);
	const granularity =
		(url.searchParams.get('granularity') as Granularity | null) ?? defaultGranularity(range);
	const offset = Number(url.searchParams.get('offset') ?? '0') || 0;
	const drilldown = readDrilldown(url);
	const sankeyDimension = readSankeyDimension(url);
	// A bucket click narrows the transaction list to that bucket's own date
	// range; absent, the list covers the whole selected period.
	const txStart = url.searchParams.get('txStart') ?? range.start;
	const txEnd = url.searchParams.get('txEnd') ?? range.end;

	const client = createGraphqlClient(fetch);
	const panel = parsePanelFilters(url.searchParams);
	const sort = parseSort(url.searchParams);

	const [accountsResult, categoriesResult, tagsResult, attentionResult] = await Promise.all([
		client.query(ACCOUNTS_QUERY, {}).toPromise(),
		client.query(CATEGORIES_QUERY, {}).toPromise(),
		client.query(TAGS_QUERY, {}).toPromise(),
		// All-time: takes no period, so it never hides a backlog outside the period.
		client.query(ATTENTION_SUMMARY_QUERY, {}).toPromise()
	]);
	const categories = (categoriesResult.data?.categories ?? []) as Category[];

	// The panel narrows the charts and the list alike, so the totals agree with
	// the list. The chart selection and the bucket's own date range then narrow
	// the list further, and only the list.
	const panelFilter = toTransactionFilter(panel, categories);
	const periodFilter = { startDate: range.start, endDate: range.end, ...panelFilter };
	const transactionFilter: TransactionFilter = {
		...layerSelection(
			{ ...panelFilter },
			Object.fromEntries(Object.entries(drilldown).filter(([, v]) => v !== undefined))
		),
		startDate: txStart,
		endDate: txEnd
	};

	const [
		summaryResult,
		graphResult,
		transactionsResult,
		breakdownResult,
		savingsBreakdownResult,
		reimbursementResult
	] = await Promise.all([
		client.query(CASHFLOW_SUMMARY_QUERY, { filter: periodFilter, granularity }).toPromise(),
		client
			.query(CASHFLOW_GRAPH_QUERY, {
				filter: periodFilter,
				grouping: groupingForDimension(sankeyDimension)
			})
			.toPromise(),
		client
			.query(TRANSACTIONS_QUERY, {
				filter: transactionFilter,
				page: { limit: PAGE_LIMIT, offset },
				sort: sortVariable(sort)
			})
			.toPromise(),
		client
			.query(CATEGORY_BREAKDOWN_QUERY, {
				filter: periodFilter,
				level: 1,
				// "Spending by category" means spending. Without a kind the
				// resolver returns every kind but transfer, so income
				// categories were showing up under a spending card, and
				// savings count as saving rather than spending (requirements,
				// "Categories (iteration 2)").
				kind: 'EXPENSE'
			})
			.toPromise(),
		// Savings is its own card: a `kind: SAVING` category is money put aside,
		// not consumed, so folding it into the spending card would inflate
		// spending with money the user still has. Asking for EXPENSE alone was
		// why savings was invisible everywhere despite being labelled.
		client
			.query(CATEGORY_BREAKDOWN_QUERY, {
				filter: periodFilter,
				level: 1,
				kind: 'SAVING'
			})
			.toPromise(),
		// The netted figure beside the totals; its failure never fails the dashboard.
		client.query(REIMBURSEMENT_SUMMARY_QUERY, { filter: periodFilter }).toPromise()
	]);

	const error = [accountsResult, summaryResult, graphResult, transactionsResult].some(
		(r) => r.error
	);

	return {
		preset,
		start: range.start,
		end: range.end,
		txStart,
		txEnd,
		granularity,
		drilldown,
		panel,
		sort,
		/** The period + panel filter the breakdown ran with; expanding a row scopes a copy of it. */
		breakdownFilter: periodFilter as Partial<TransactionFilter>,
		sankeyDimension,
		offset,
		error,
		accounts: (accountsResult.data?.accounts ?? []) as Account[],
		categories,
		attention: attentionResult.data?.attentionSummary as AttentionSummary | undefined,
		allTags: (tagsResult.data?.tags ?? []) as TagCount[],
		summary: summaryResult.data?.cashflowSummary as CashflowSummary | undefined,
		graph: graphResult.data?.cashflowGraph as CashflowGraph | undefined,
		transactionFilter,
		transactions: transactionsResult.data?.transactions as TransactionPage | undefined,
		breakdown: breakdownResult.data?.categoryBreakdown as CategoryBreakdown | undefined,
		/** `kind: SAVING` rows for the same period; its failure never fails the dashboard. */
		savingsBreakdown: savingsBreakdownResult.data?.categoryBreakdown as
			| CategoryBreakdown
			| undefined,
		reimbursementSummary: reimbursementResult.data?.reimbursementSummary as
			| ReimbursementSummary
			| undefined,
		today: toDateInputValue(today)
	};
};
