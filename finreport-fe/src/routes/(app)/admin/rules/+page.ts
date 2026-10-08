import { createGraphqlClient } from '$lib/graphqlClient';
import {
	CATEGORIES_QUERY,
	LEARNING_EXEMPTIONS_QUERY,
	RECENTLY_AUTO_APPROVED_RULES_QUERY,
	RULES_QUERY
} from '$lib/graphql/adminRulesQueries';
import type { Category, LearningExemption, Rule } from '$lib/graphql/types';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ fetch }) => {
	const client = createGraphqlClient(fetch);

	const [rulesResult, recentResult, categoriesResult, exemptionsResult] = await Promise.all([
		client.query(RULES_QUERY, {}).toPromise(),
		client.query(RECENTLY_AUTO_APPROVED_RULES_QUERY, { limit: 10 }).toPromise(),
		client.query(CATEGORIES_QUERY, { includeArchived: false }).toPromise(),
		client.query(LEARNING_EXEMPTIONS_QUERY, {}).toPromise()
	]);

	return {
		error: Boolean(
			rulesResult.error || recentResult.error || categoriesResult.error || exemptionsResult.error
		),
		rules: (rulesResult.data?.rules ?? []) as Rule[],
		recentlyAutoApproved: (recentResult.data?.recentlyAutoApprovedRules ?? []) as Rule[],
		categories: (categoriesResult.data?.categories ?? []) as Category[],
		exemptions: (exemptionsResult.data?.learningExemptions ?? []) as LearningExemption[]
	};
};
