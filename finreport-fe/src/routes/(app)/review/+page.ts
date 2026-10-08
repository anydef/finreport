import { createGraphqlClient } from '$lib/graphqlClient';
import {
	CATEGORIES_QUERY,
	REVIEW_HELD_TRANSACTIONS_QUERY,
	REVIEW_QUEUE_WITH_SPLITS_QUERY
} from '$lib/graphql/adminReviewQueries';
import { reviewQueueFilter } from '$lib/bulkSelection';
import type { Category, ReviewQueue, Transaction } from '$lib/graphql/types';
import type { PageLoad } from './$types';

const PAGE_LIMIT = 50;

/**
 * `?counterpartyKey=` narrows the queue to the held transactions of one
 * counterparty ("find similar"). `reviewQueue` takes no filter, so that case
 * reads `transactions(needsReview, counterpartyKeys)` for the rows and still
 * calls `reviewQueue` (one row) for the pending rules.
 */
export const load: PageLoad = async ({ fetch, url }) => {
	const offset = Number(url.searchParams.get('offset') ?? '0') || 0;
	const counterpartyKey = url.searchParams.get('counterpartyKey') || null;
	const client = createGraphqlClient(fetch);

	const [reviewQueueResult, categoriesResult, similarResult] = await Promise.all([
		client
			.query(REVIEW_QUEUE_WITH_SPLITS_QUERY, {
				page: counterpartyKey ? { limit: 1, offset: 0 } : { limit: PAGE_LIMIT, offset }
			})
			.toPromise(),
		client.query(CATEGORIES_QUERY, { includeArchived: false }).toPromise(),
		counterpartyKey
			? client
					.query(REVIEW_HELD_TRANSACTIONS_QUERY, {
						filter: reviewQueueFilter(counterpartyKey),
						page: { limit: PAGE_LIMIT, offset }
					})
					.toPromise()
			: Promise.resolve(null)
	]);

	const queue = reviewQueueResult.data?.reviewQueue as ReviewQueue | undefined;
	const similar = similarResult?.data?.transactions as
		| { items: Transaction[]; totalCount: number }
		| undefined;
	const held = counterpartyKey
		? similar && { transactions: similar.items, totalCount: similar.totalCount }
		: queue && { transactions: queue.transactions, totalCount: queue.totalCount };

	return {
		offset,
		limit: PAGE_LIMIT,
		counterpartyKey,
		error: Boolean(reviewQueueResult.error || categoriesResult.error || similarResult?.error),
		reviewQueue: queue && held ? ({ ...queue, ...held } as ReviewQueue) : undefined,
		categories: (categoriesResult.data?.categories ?? []) as Category[]
	};
};
