import { createGraphqlClient } from '$lib/graphqlClient';
import {
	CATEGORIES_QUERY,
	GOAL_PROGRESS_QUERY,
	GOAL_TRANSACTIONS_QUERY
} from '$lib/graphql/queries';
import { progressRange } from '$lib/goalsView';
import type { Category, GoalProgress, TransactionPage } from '$lib/graphql/types';
import { DEFAULT_SORT, parseSort, sortVariable } from '$lib/transactionSort';
import type { PageLoad } from './$types';

const PAGE_LIMIT = 50;
/** One generous page of the whole range, to draw a fixed goal's cumulative line. */
const SERIES_LIMIT = 200;

export const load: PageLoad = async ({ fetch, params, url }) => {
	const client = createGraphqlClient(fetch);

	const [progressResult, categoriesResult] = await Promise.all([
		client.query(GOAL_PROGRESS_QUERY, { id: params.id }).toPromise(),
		client.query(CATEGORIES_QUERY, { includeArchived: false }).toPromise()
	]);
	const progress = progressResult.data?.goalProgress as GoalProgress | undefined;
	const categories = (categoriesResult.data?.categories ?? []) as Category[];

	if (!progress) {
		return { error: true, id: params.id, categories, sort: DEFAULT_SORT } as const;
	}

	// A bucket click narrows the list to that bucket's range (the dashboard's
	// `txStart`/`txEnd` pattern); absent, the list covers every bucket.
	const range = progressRange(progress.buckets) ?? { start: '', end: '' };
	const txStart = url.searchParams.get('txStart') ?? range.start;
	const txEnd = url.searchParams.get('txEnd') ?? range.end;
	const offset = Number(url.searchParams.get('offset') ?? '0') || 0;
	const sort = parseSort(url.searchParams);
	const fixed = progress.goal.periodKind === 'FIXED';

	const [transactionsResult, seriesResult] = await Promise.all([
		client
			.query(GOAL_TRANSACTIONS_QUERY, {
				id: params.id,
				startDate: txStart,
				endDate: txEnd,
				page: { limit: PAGE_LIMIT, offset },
				sort: sortVariable(sort)
			})
			.toPromise(),
		fixed
			? client
					.query(GOAL_TRANSACTIONS_QUERY, {
						id: params.id,
						startDate: range.start,
						endDate: range.end,
						page: { limit: SERIES_LIMIT, offset: 0 }
					})
					.toPromise()
			: Promise.resolve(null)
	]);

	return {
		error: false,
		id: params.id,
		progress,
		categories,
		range,
		txStart,
		txEnd,
		sort,
		offset,
		transactions: transactionsResult.data?.goalTransactions as TransactionPage | undefined,
		transactionsError: Boolean(transactionsResult.error),
		seriesTransactions: seriesResult?.data?.goalTransactions as TransactionPage | undefined
	} as const;
};
