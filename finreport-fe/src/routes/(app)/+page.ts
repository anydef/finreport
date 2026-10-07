import { createGraphqlClient } from '$lib/graphqlClient';
import {
	ACCOUNTS_QUERY,
	CASHFLOW_GRAPH_QUERY,
	CASHFLOW_SUMMARY_QUERY,
	CATEGORIES_QUERY,
	CATEGORY_BREAKDOWN_QUERY,
	TAGS_QUERY,
	TRANSACTIONS_QUERY
} from '$lib/graphql/queries';
import type {
	Account,
	CashflowDimension,
	CashflowGraph,
	CashflowSummary,
	Category,
	CategoryBreakdown,
	TagCount,
	TransactionFilter,
	TransactionPage
} from '$lib/graphql/types';
import {
	defaultGranularity,
	presetRange,
	toDateInputValue,
	type DateRange,
	type Granularity,
	type PeriodPresetId
} from '$lib/period';
import { layerSelection, parsePanelFilters, toTransactionFilter } from '$lib/transactionFilters';
import type { PageLoad } from './$types';

const PAGE_LIMIT = 50;

function readRange(url: URL, preset: PeriodPresetId, today: Date): DateRange {
	if (preset === 'custom') {
		const fallback = presetRange('this-month', today);
		return {
			start: url.searchParams.get('start') ?? fallback.start,
			end: url.searchParams.get('end') ?? fallback.end
		};
	}
	return presetRange(preset, today);
}

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
	const uncategorizedParam = url.searchParams.get('selUncategorized');
	return {
		accountIds: accountIds?.length ? accountIds : undefined,
		counterpartyNames: counterpartyNames?.length ? counterpartyNames : undefined,
		hasCounterparty: hasCounterpartyParam === 'false' ? false : undefined,
		categorySlugs: categorySlugs?.length ? categorySlugs : undefined,
		uncategorized: uncategorizedParam === 'true' ? true : undefined
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
	const preset = (url.searchParams.get('preset') as PeriodPresetId | null) ?? 'this-month';
	const range = readRange(url, preset, today);
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

	const [accountsResult, categoriesResult, tagsResult] = await Promise.all([
		client.query(ACCOUNTS_QUERY, {}).toPromise(),
		client.query(CATEGORIES_QUERY, {}).toPromise(),
		client.query(TAGS_QUERY, {}).toPromise()
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

	const [summaryResult, graphResult, transactionsResult, breakdownResult] = await Promise.all([
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
				page: { limit: PAGE_LIMIT, offset }
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
			.toPromise()
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
		sankeyDimension,
		offset,
		error,
		accounts: (accountsResult.data?.accounts ?? []) as Account[],
		categories,
		allTags: (tagsResult.data?.tags ?? []) as TagCount[],
		summary: summaryResult.data?.cashflowSummary as CashflowSummary | undefined,
		graph: graphResult.data?.cashflowGraph as CashflowGraph | undefined,
		transactionFilter,
		transactions: transactionsResult.data?.transactions as TransactionPage | undefined,
		breakdown: breakdownResult.data?.categoryBreakdown as CategoryBreakdown | undefined,
		today: toDateInputValue(today)
	};
};
