import { createGraphqlClient } from '$lib/graphqlClient';
import { CATEGORIES_QUERY, REVIEW_QUEUE_WITH_SPLITS_QUERY } from '$lib/graphql/adminReviewQueries';
import type { Category, ReviewQueue } from '$lib/graphql/types';
import type { PageLoad } from './$types';

const PAGE_LIMIT = 50;

export const load: PageLoad = async ({ fetch, url }) => {
	const offset = Number(url.searchParams.get('offset') ?? '0') || 0;
	const client = createGraphqlClient(fetch);

	const [reviewQueueResult, categoriesResult] = await Promise.all([
		client
			.query(REVIEW_QUEUE_WITH_SPLITS_QUERY, { page: { limit: PAGE_LIMIT, offset } })
			.toPromise(),
		client.query(CATEGORIES_QUERY, { includeArchived: false }).toPromise()
	]);

	return {
		offset,
		error: Boolean(reviewQueueResult.error || categoriesResult.error),
		reviewQueue: reviewQueueResult.data?.reviewQueue as ReviewQueue | undefined,
		categories: (categoriesResult.data?.categories ?? []) as Category[]
	};
};
