import { createGraphqlClient } from '$lib/graphqlClient';
import { PURCHASE_ORDER_MATCHES_QUERY } from '$lib/graphql/adminAmazonQueries';
import type { PurchaseOrderMatchReport } from '$lib/graphql/types';
import type { PageLoad } from './$types';

const EMPTY: PurchaseOrderMatchReport = {
	orderCount: 0,
	matches: [],
	unmatchedOrders: [],
	unmatchedCharges: [],
	subscriptionCharges: []
};

export const load: PageLoad = async ({ fetch }) => {
	const result = await createGraphqlClient(fetch)
		.query(PURCHASE_ORDER_MATCHES_QUERY, {})
		.toPromise();
	return {
		error: Boolean(result.error),
		report: (result.data?.purchaseOrderMatches ?? EMPTY) as PurchaseOrderMatchReport
	};
};
