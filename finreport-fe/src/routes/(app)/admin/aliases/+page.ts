import { createGraphqlClient } from '$lib/graphqlClient';
import { DISPLAY_ALIASES_QUERY } from '$lib/graphql/adminAliasesQueries';
import { ACCOUNTS_QUERY } from '$lib/graphql/queries';
import type { Account, DisplayAlias } from '$lib/graphql/types';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ fetch }) => {
	const client = createGraphqlClient(fetch);

	const [aliasesResult, accountsResult] = await Promise.all([
		client.query(DISPLAY_ALIASES_QUERY, {}).toPromise(),
		client.query(ACCOUNTS_QUERY, {}).toPromise()
	]);

	return {
		error: Boolean(aliasesResult.error || accountsResult.error),
		aliases: (aliasesResult.data?.displayAliases ?? []) as DisplayAlias[],
		accounts: (accountsResult.data?.accounts ?? []) as Account[]
	};
};
