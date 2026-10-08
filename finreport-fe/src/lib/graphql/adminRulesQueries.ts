/**
 * GraphQL documents for `/admin/rules` and `/admin/categories` (§5, §10
 * WP6), re-exported from the frozen `queries.ts` (WP0-owned, §10 shared-file
 * protocol) under names scoped to this screen so the routes/components only
 * need one import. Everything is re-exported from `queries.ts` except the
 * learning-exemption documents, which live here.
 */

export {
	RULES_QUERY,
	RECENTLY_AUTO_APPROVED_RULES_QUERY,
	CREATE_RULE_MUTATION,
	UPDATE_RULE_MUTATION,
	SET_RULE_STATE_MUTATION,
	REAPPLY_RULE_MUTATION,
	CATEGORIES_QUERY,
	CREATE_CATEGORY_MUTATION,
	RENAME_CATEGORY_MUTATION,
	ARCHIVE_CATEGORY_MUTATION
} from './queries';

const LEARNING_EXEMPTION_FIELDS = `
	counterpartyKey
	displayName
	transactionCount
	exemptedAt`;

export const LEARNING_EXEMPTIONS_QUERY = `
query LearningExemptions {
	learningExemptions {${LEARNING_EXEMPTION_FIELDS}
	}
}`;

export const EXEMPT_FROM_LEARNING_MUTATION = `
mutation ExemptFromLearning($counterpartyKey: String!) {
	exemptFromLearning(counterpartyKey: $counterpartyKey) {${LEARNING_EXEMPTION_FIELDS}
	}
}`;

export const REMOVE_LEARNING_EXEMPTION_MUTATION = `
mutation RemoveLearningExemption($counterpartyKey: String!) {
	removeLearningExemption(counterpartyKey: $counterpartyKey)
}`;
