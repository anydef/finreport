import { createGraphqlClient } from '$lib/graphqlClient';
import {
	ACCOUNTS_QUERY,
	CATEGORIES_QUERY,
	TAGS_QUERY,
	TRANSACTIONS_QUERY
} from '$lib/graphql/queries';
import type { Account, Category, TagCount, TransactionPage } from '$lib/graphql/types';
import {
	defaultGranularity,
	presetRange,
	toDateInputValue,
	type DateRange,
	type PeriodPresetId
} from '$lib/period';
import { parsePanelFilters, toTransactionFilter } from '$lib/transactionFilters';
import { parseSort, sortVariable } from '$lib/transactionSort';
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

export const load: PageLoad = async ({ fetch, url }) => {
	const today = new Date();
	const preset = (url.searchParams.get('preset') as PeriodPresetId | null) ?? 'this-month';
	const range = readRange(url, preset, today);
	const offset = Number(url.searchParams.get('offset') ?? '0') || 0;
	const panel = parsePanelFilters(url.searchParams);
	const sort = parseSort(url.searchParams);

	const client = createGraphqlClient(fetch);

	const [accountsResult, categoriesResult, tagsResult] = await Promise.all([
		client.query(ACCOUNTS_QUERY, {}).toPromise(),
		client.query(CATEGORIES_QUERY, {}).toPromise(),
		client.query(TAGS_QUERY, {}).toPromise()
	]);
	const categories = (categoriesResult.data?.categories ?? []) as Category[];

	const filter = {
		startDate: range.start,
		endDate: range.end,
		...toTransactionFilter(panel, categories)
	};

	const transactionsResult = await client
		.query(TRANSACTIONS_QUERY, {
			filter,
			page: { limit: PAGE_LIMIT, offset },
			sort: sortVariable(sort)
		})
		.toPromise();

	return {
		preset,
		start: range.start,
		end: range.end,
		granularity: defaultGranularity(range),
		panel,
		sort,
		offset,
		error: Boolean(
			accountsResult.error || categoriesResult.error || tagsResult.error || transactionsResult.error
		),
		accounts: (accountsResult.data?.accounts ?? []) as Account[],
		categories,
		allTags: (tagsResult.data?.tags ?? []) as TagCount[],
		transactionFilter: filter,
		transactions: transactionsResult.data?.transactions as TransactionPage | undefined,
		today: toDateInputValue(today)
	};
};
