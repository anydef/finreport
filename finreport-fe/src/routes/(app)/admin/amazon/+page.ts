import { createGraphqlClient } from '$lib/graphqlClient';
import { PURCHASE_ORDER_MATCHES_QUERY } from '$lib/graphql/adminAmazonQueries';
import { CATEGORIES_QUERY } from '$lib/graphql/queries';
import type { Category, PurchaseOrderMatchReport } from '$lib/graphql/types';
import type { PageLoad } from './$types';

const EMPTY: PurchaseOrderMatchReport = {
	orderCount: 0,
	matches: [],
	unmatchedOrders: [],
	unmatchedCharges: [],
	subscriptionCharges: []
};

export const load: PageLoad = async ({ fetch }) => {
	const client = createGraphqlClient(fetch);
	const [matches, categories] = await Promise.all([
		client.query(PURCHASE_ORDER_MATCHES_QUERY, {}).toPromise(),
		client.query(CATEGORIES_QUERY, {}).toPromise()
	]);
	return {
		error: Boolean(matches.error),
		report: (matches.data?.purchaseOrderMatches ?? EMPTY) as PurchaseOrderMatchReport,
		categories: (categories.data?.categories ?? []) as Category[]
	};
};
