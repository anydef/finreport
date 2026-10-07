import { createGraphqlClient } from '$lib/graphqlClient';
import { CATEGORIES_QUERY } from '$lib/graphql/adminRulesQueries';
import type { Category } from '$lib/graphql/types';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ fetch }) => {
	const client = createGraphqlClient(fetch);
	const result = await client.query(CATEGORIES_QUERY, { includeArchived: true }).toPromise();

	return {
		error: Boolean(result.error),
		categories: (result.data?.categories ?? []) as Category[]
	};
};
