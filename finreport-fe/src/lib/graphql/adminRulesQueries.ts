/**
 * GraphQL documents for `/admin/rules` and `/admin/categories` (§5, §10
 * WP6), re-exported from the frozen `queries.ts` (WP0-owned, §10 shared-file
 * protocol) under names scoped to this screen so the routes/components only
 * need one import. No new operations are defined here — every document
 * already exists in `queries.ts`.
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
