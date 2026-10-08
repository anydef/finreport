import { createGraphqlClient } from '$lib/graphqlClient';
import {
	CATEGORIES_QUERY,
	HELD_MERCHANT_GROUPS_QUERY,
	REVIEW_HELD_TRANSACTIONS_QUERY,
	REVIEW_QUEUE_WITH_SPLITS_QUERY
} from '$lib/graphql/adminReviewQueries';
import { reviewQueueFilter } from '$lib/bulkSelection';
import { expandedKey, parseView } from '$lib/heldGroups';
import type { Category, HeldMerchantGroups, ReviewQueue, Transaction } from '$lib/graphql/types';
import type { PageLoad } from './$types';

const PAGE_LIMIT = 50;
// Groups are far fewer than transactions (one per merchant), so one page of the
// maximum size holds the whole grouped queue in practice.
const GROUP_LIMIT = 200;

/**
 * `?view=groups` (default): the held queue grouped by merchant, plus, when
 * `?expand=<counterpartyKey>` names a group, that group's held transactions.
 * `?view=flat`: the paged flat list. The pre-grouping `?counterpartyKey=` link
 * is read as `expand`, so old links land on the merchant's expanded group.
 */
export const load: PageLoad = async ({ fetch, url }) => {
	const view = parseView(url.searchParams.get('view'));
	const offset = Number(url.searchParams.get('offset') ?? '0') || 0;
	const expand = view === 'groups' ? expandedKey(url.searchParams) : null;
	const client = createGraphqlClient(fetch);

	const [queueResult, categoriesResult, groupsResult, expandedResult] = await Promise.all([
		client
			.query(REVIEW_QUEUE_WITH_SPLITS_QUERY, {
				page: view === 'flat' ? { limit: PAGE_LIMIT, offset } : { limit: 1, offset: 0 }
			})
			.toPromise(),
		client.query(CATEGORIES_QUERY, { includeArchived: false }).toPromise(),
		view === 'groups'
			? client
					.query(HELD_MERCHANT_GROUPS_QUERY, { page: { limit: GROUP_LIMIT, offset: 0 } })
					.toPromise()
			: Promise.resolve(null),
		expand
			? client
					.query(REVIEW_HELD_TRANSACTIONS_QUERY, {
						filter: reviewQueueFilter(expand),
						page: { limit: GROUP_LIMIT, offset: 0 }
					})
					.toPromise()
			: Promise.resolve(null)
	]);

	return {
		view,
		offset,
		limit: PAGE_LIMIT,
		expand,
		error: Boolean(
			queueResult.error || categoriesResult.error || groupsResult?.error || expandedResult?.error
		),
		reviewQueue: queueResult.data?.reviewQueue as ReviewQueue | undefined,
		groups: groupsResult?.data?.heldMerchantGroups as HeldMerchantGroups | undefined,
		expandedTransactions: (expandedResult?.data?.transactions?.items ?? []) as Transaction[],
		categories: (categoriesResult.data?.categories ?? []) as Category[]
	};
};
