import { createGraphqlClient } from '$lib/graphqlClient';
import { RECURRING_SERIES_QUERY } from '$lib/graphql/queries';
import type { RecurringOverview } from '$lib/graphql/types';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ fetch }) => {
	const client = createGraphqlClient(fetch);
	const result = await client.query(RECURRING_SERIES_QUERY, { filter: undefined }).toPromise();

	return {
		error: Boolean(result.error),
		overview: result.data?.recurringSeries as RecurringOverview | undefined
	};
};
