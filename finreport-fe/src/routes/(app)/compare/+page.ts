import { createGraphqlClient } from '$lib/graphqlClient';
import { CATEGORY_COMPARISON_QUERY } from '$lib/graphql/queries';
import {
	comparisonWindow,
	defaultPair,
	parseSpan,
	periodIndexByStart,
	shapeComparison,
	type CategoryComparison
} from '$lib/comparisonView';
import { toDateInputValue } from '$lib/period';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ fetch, url }) => {
	const today = new Date();
	const span = parseSpan(url.searchParams.get('months'));
	const window = comparisonWindow(span, url.searchParams.get('end'), today);

	const client = createGraphqlClient(fetch);
	const result = await client
		.query(CATEGORY_COMPARISON_QUERY, {
			filter: { startDate: window.range.start, endDate: window.range.end },
			granularity: 'MONTH'
		})
		.toPromise();

	const comparison = result.data?.categoryComparison as CategoryComparison | undefined;
	const view = comparison
		? (() => {
				const pair = defaultPair(comparison.periods.length, window.partial);
				return shapeComparison(
					comparison,
					periodIndexByStart(comparison.periods, url.searchParams.get('base')) ?? pair.base,
					periodIndexByStart(comparison.periods, url.searchParams.get('to')) ?? pair.current,
					window.partial
				);
			})()
		: undefined;

	return {
		span,
		window,
		view,
		error: Boolean(result.error),
		today: toDateInputValue(today)
	};
};
