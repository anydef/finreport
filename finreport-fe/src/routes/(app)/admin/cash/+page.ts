import { createGraphqlClient } from '$lib/graphqlClient';
import { ACCOUNTS_QUERY } from '$lib/graphql/queries';
import type { Account } from '$lib/graphql/types';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ fetch }) => {
	const result = await createGraphqlClient(fetch).query(ACCOUNTS_QUERY, {}).toPromise();
	return {
		error: Boolean(result.error),
		accounts: (result.data?.accounts ?? []) as Account[]
	};
};
