import { createGraphqlClient } from '$lib/graphqlClient';
import {
	ACCOUNTS_QUERY,
	CASHFLOW_GRAPH_QUERY,
	CASHFLOW_SUMMARY_QUERY,
	TRANSACTIONS_QUERY
} from '$lib/graphql/queries';
import type { Account, CashflowGraph, CashflowSummary, TransactionPage } from '$lib/graphql/types';
import {
	defaultGranularity,
	presetRange,
	toDateInputValue,
	type DateRange,
	type Granularity,
	type PeriodPresetId
} from '$lib/period';
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

/** Drill-down narrowing applied only to the transaction list (§6), not the charts. */
function readDrilldown(url: URL) {
	const accountIds = url.searchParams.get('accountIds')?.split(',').filter(Boolean);
	const counterpartyNames = url.searchParams.get('counterpartyNames')?.split(',').filter(Boolean);
	const hasCounterpartyParam = url.searchParams.get('hasCounterparty');
	return {
		accountIds: accountIds?.length ? accountIds : undefined,
		counterpartyNames: counterpartyNames?.length ? counterpartyNames : undefined,
		hasCounterparty: hasCounterpartyParam === 'false' ? false : undefined
	};
}

export const load: PageLoad = async ({ fetch, url }) => {
	const today = new Date();
	const preset = (url.searchParams.get('preset') as PeriodPresetId | null) ?? 'this-month';
	const range = readRange(url, preset, today);
	const granularity =
		(url.searchParams.get('granularity') as Granularity | null) ?? defaultGranularity(range);
	const offset = Number(url.searchParams.get('offset') ?? '0') || 0;
	const drilldown = readDrilldown(url);
	// A bucket click narrows the transaction list to that bucket's own date
	// range; absent, the list covers the whole selected period.
	const txStart = url.searchParams.get('txStart') ?? range.start;
	const txEnd = url.searchParams.get('txEnd') ?? range.end;

	const client = createGraphqlClient(fetch);
	const periodFilter = { startDate: range.start, endDate: range.end };
	const transactionFilter = { startDate: txStart, endDate: txEnd, ...drilldown };

	const [accountsResult, summaryResult, graphResult, transactionsResult] = await Promise.all([
		client.query(ACCOUNTS_QUERY, {}).toPromise(),
		client.query(CASHFLOW_SUMMARY_QUERY, { filter: periodFilter, granularity }).toPromise(),
		client.query(CASHFLOW_GRAPH_QUERY, { filter: periodFilter, grouping: null }).toPromise(),
		client
			.query(TRANSACTIONS_QUERY, {
				filter: transactionFilter,
				page: { limit: PAGE_LIMIT, offset }
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
		offset,
		error,
		accounts: (accountsResult.data?.accounts ?? []) as Account[],
		summary: summaryResult.data?.cashflowSummary as CashflowSummary | undefined,
		graph: graphResult.data?.cashflowGraph as CashflowGraph | undefined,
		transactions: transactionsResult.data?.transactions as TransactionPage | undefined,
		today: toDateInputValue(today)
	};
};
