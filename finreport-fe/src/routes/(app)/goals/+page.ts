import { createGraphqlClient } from '$lib/graphqlClient';
import { CATEGORIES_QUERY, GOAL_PROGRESS_QUERY, GOALS_QUERY } from '$lib/graphql/queries';
import type { Category, Goal, GoalProgress } from '$lib/graphql/types';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ fetch, url }) => {
	const client = createGraphqlClient(fetch);
	const includeArchived = url.searchParams.get('archived') === 'true';

	const [goalsResult, categoriesResult] = await Promise.all([
		client.query(GOALS_QUERY, { includeArchived }).toPromise(),
		client.query(CATEGORIES_QUERY, { includeArchived: false }).toPromise()
	]);
	const goals = (goalsResult.data?.goals ?? []) as Goal[];

	// One progress query per goal: the card shows the current period only. A
	// failed progress query leaves that card without a bar rather than
	// failing the whole page.
	const progressResults = await Promise.all(
		goals.map((goal) => client.query(GOAL_PROGRESS_QUERY, { id: goal.id }).toPromise())
	);
	const progress: Record<string, GoalProgress | undefined> = {};
	goals.forEach((goal, i) => {
		progress[goal.id] = progressResults[i].data?.goalProgress as GoalProgress | undefined;
	});

	return {
		error: Boolean(goalsResult.error),
		includeArchived,
		goals,
		progress,
		categories: (categoriesResult.data?.categories ?? []) as Category[]
	};
};
