/**
 * GraphQL operation documents against the frozen SDL (`schema.graphql`).
 * Plain strings — `@urql/core`'s `query()`/`mutation()` accept a string or a
 * `DocumentNode`, no `gql` tag needed.
 */

export const ME_QUERY = `
query Me {
	me {
		id
		username
		displayName
		isAdmin
	}
}`;

export const LOGIN_MUTATION = `
mutation Login($input: LoginInput!) {
	login(input: $input) {
		id
		username
		displayName
		isAdmin
	}
}`;

export const LOGOUT_MUTATION = `
mutation Logout {
	logout
}`;

export const ACCOUNTS_QUERY = `
query Accounts {
	accounts {
		id
		source
		externalId
		displayId
		accountType
		iban
		bic
		institute
		label
		currency
		latestBalance {
			date
			amount
			currency
		}
	}
}`;

/** The `Transaction` selection shared by every query that returns a `TransactionPage`. */
const TRANSACTION_ITEM_FIELDS = `			id
			accountId
			source
			externalId
			bookingDate
			valutaDate
			bookingStatus
			amount
			currency
			counterpartyName
			counterpartyIban
			description
			transactionType
			label {
				category {
					id
					slug
					name
					kind
				}
				source
				confidence
				status
				reviewReason
				proposedCategoryPath
				reasoning
			}
			splits {
				index
				amount
				category {
					id
					slug
					name
					kind
				}
			}
			tags
			transfer {
				counterpartTransactionId
				counterpartAccountId
				match
			}
			recurring {
				isRecurring
				source
				seriesId
				cadence
				medianAmount
			}`;

// `tags`/`transfer`/`recurring` (iteration 3 §4) were deliberately left off
// this query by WP0 until WP-B's resolvers were real (§9 WP0 addendum #5);
// WP-C adds them here now that the mocks model them.
export const TRANSACTIONS_QUERY = `
query Transactions($filter: TransactionFilter, $page: PageInput) {
	transactions(filter: $filter, page: $page) {
		items {
${TRANSACTION_ITEM_FIELDS}
		}
		totalCount
		limit
		offset
	}
}`;

export const CASHFLOW_SUMMARY_QUERY = `
query CashflowSummary($filter: TransactionFilter!, $granularity: Granularity!) {
	cashflowSummary(filter: $filter, granularity: $granularity) {
		buckets {
			start
			end
			income
			spending
			net
			transactionCount
		}
		total {
			income
			spending
			net
			transactionCount
		}
		currency
	}
}`;

export const CASHFLOW_GRAPH_QUERY = `
query CashflowGraph($filter: TransactionFilter!, $grouping: CashflowGraphInput) {
	cashflowGraph(filter: $filter, grouping: $grouping) {
		nodes {
			id
			label
			kind
			depth
			value
			refType
			refId
		}
		links {
			sourceId
			targetId
			value
		}
		currency
		dimensions
		truncated
	}
}`;

// ---------------------------------------------------------------------------
// Categories, labels, rules (iteration 2, §5)
// ---------------------------------------------------------------------------

export const CATEGORIES_QUERY = `
query Categories($includeArchived: Boolean! = false) {
	categories(includeArchived: $includeArchived) {
		id
		slug
		name
		kind
		parentId
		depth
		archived
		origin
	}
}`;

export const CATEGORY_BREAKDOWN_QUERY = `
query CategoryBreakdown($filter: TransactionFilter!, $level: Int! = 1, $kind: CategoryKind) {
	categoryBreakdown(filter: $filter, level: $level, kind: $kind) {
		rows {
			category {
				id
				slug
				name
				kind
			}
			amount
			transactionCount
			share
		}
		uncategorized {
			category {
				id
				slug
				name
				kind
			}
			amount
			transactionCount
			share
		}
		needsReview {
			category {
				id
				slug
				name
				kind
			}
			amount
			transactionCount
			share
		}
		currency
	}
}`;

const RULE_FIELDS = `
	id
	name
	category {
		id
		slug
		name
		kind
	}
	conditions
	priority
	state
	origin
	autoApproved
	confidence
	evidenceCount
	createdAt`;

export const RULES_QUERY = `
query Rules($state: RuleState) {
	rules(state: $state) {${RULE_FIELDS}
	}
}`;

export const RECENTLY_AUTO_APPROVED_RULES_QUERY = `
query RecentlyAutoApprovedRules($limit: Int! = 20) {
	recentlyAutoApprovedRules(limit: $limit) {${RULE_FIELDS}
	}
}`;

export const REVIEW_QUEUE_QUERY = `
query ReviewQueue($page: PageInput) {
	reviewQueue(page: $page) {
		transactions {
			id
			accountId
			bookingDate
			amount
			currency
			counterpartyName
			description
			label {
				source
				status
				reviewReason
				proposedCategoryPath
				reasoning
				confidence
			}
		}
		pendingRules {${RULE_FIELDS}
		}
		totalCount
	}
}`;

export const SET_TRANSACTION_CATEGORY_MUTATION = `
mutation SetTransactionCategory($transactionId: UUID!, $categorySlug: String!) {
	setTransactionCategory(transactionId: $transactionId, categorySlug: $categorySlug) {
		id
		label {
			category {
				id
				slug
				name
				kind
			}
			source
			status
		}
	}
}`;

export const CLEAR_TRANSACTION_CATEGORY_MUTATION = `
mutation ClearTransactionCategory($transactionId: UUID!) {
	clearTransactionCategory(transactionId: $transactionId) {
		id
		label {
			category {
				id
				slug
			}
			source
			status
		}
	}
}`;

export const SPLIT_TRANSACTION_MUTATION = `
mutation SplitTransaction($transactionId: UUID!, $parts: [SplitPartInput!]!) {
	splitTransaction(transactionId: $transactionId, parts: $parts) {
		id
		splits {
			index
			amount
			category {
				id
				slug
				name
			}
		}
	}
}`;

export const UNSPLIT_TRANSACTION_MUTATION = `
mutation UnsplitTransaction($transactionId: UUID!) {
	unsplitTransaction(transactionId: $transactionId) {
		id
		splits {
			index
		}
	}
}`;

export const CREATE_CATEGORY_MUTATION = `
mutation CreateCategory($input: CategoryInput!) {
	createCategory(input: $input) {
		id
		slug
		name
		kind
		parentId
		depth
		archived
		origin
	}
}`;

export const RENAME_CATEGORY_MUTATION = `
mutation RenameCategory($id: UUID!, $name: String!) {
	renameCategory(id: $id, name: $name) {
		id
		slug
		name
	}
}`;

export const ARCHIVE_CATEGORY_MUTATION = `
mutation ArchiveCategory($id: UUID!) {
	archiveCategory(id: $id) {
		id
		slug
		archived
	}
}`;

export const CREATE_RULE_MUTATION = `
mutation CreateRule($input: RuleInput!) {
	createRule(input: $input) {${RULE_FIELDS}
	}
}`;

export const UPDATE_RULE_MUTATION = `
mutation UpdateRule($id: UUID!, $input: RuleInput!) {
	updateRule(id: $id, input: $input) {${RULE_FIELDS}
	}
}`;

export const SET_RULE_STATE_MUTATION = `
mutation SetRuleState($id: UUID!, $state: RuleState!) {
	setRuleState(id: $id, state: $state) {${RULE_FIELDS}
	}
}`;

export const REAPPLY_RULE_MUTATION = `
mutation ReapplyRule($id: UUID!) {
	reapplyRule(id: $id)
}`;

// ---------------------------------------------------------------------------
// Tags, internal transfers, recurring costs (iteration 3, §4)
// ---------------------------------------------------------------------------

export const TAGS_QUERY = `
query Tags {
	tags {
		tag
		transactionCount
	}
}`;

export const RECURRING_SERIES_QUERY = `
query RecurringSeries($filter: TransactionFilter) {
	recurringSeries(filter: $filter) {
		series {
			id
			counterpartyKey
			counterpartyName
			direction
			cadence
			medianAmount
			monthlyEquivalent
			occurrenceCount
			firstDate
			lastDate
			nextExpectedDate
			stale
		}
		totalMonthlyEquivalent
		currency
	}
}`;

export const SET_TRANSACTION_TAGS_MUTATION = `
mutation SetTransactionTags($transactionId: UUID!, $tags: [String!]!) {
	setTransactionTags(transactionId: $transactionId, tags: $tags) {
		id
		tags
	}
}`;

export const SET_TRANSACTION_RECURRING_MUTATION = `
mutation SetTransactionRecurring($transactionId: UUID!, $recurring: Boolean) {
	setTransactionRecurring(transactionId: $transactionId, recurring: $recurring) {
		id
		recurring {
			isRecurring
			source
			seriesId
			cadence
			medianAmount
		}
	}
}`;

// ---------------------------------------------------------------------------
// Goals (iteration 4, §4)
//
// None of these fields are added to an existing query: the goal resolvers
// return NOT_IMPLEMENTED until WP-B lands, which would fail a whole query
// against a live backend (iteration 3 §9.5).
// ---------------------------------------------------------------------------

const GOAL_FIELDS = `
	id
	name
	type
	amount
	currency
	scope {
		categories {
			id
			slug
			name
			kind
			parentId
			depth
			archived
			origin
		}
		tags
		combine
		tagCombine
	}
	periodKind
	cadence
	startDate
	endDate
	archived`;

export const GOALS_QUERY = `
query Goals($includeArchived: Boolean! = false) {
	goals(includeArchived: $includeArchived) {${GOAL_FIELDS}
	}
}`;

export const GOAL_QUERY = `
query Goal($id: UUID!) {
	goal(id: $id) {${GOAL_FIELDS}
	}
}`;

export const GOAL_PROGRESS_QUERY = `
query GoalProgress($id: UUID!, $startDate: Date, $endDate: Date) {
	goalProgress(id: $id, startDate: $startDate, endDate: $endDate) {
		goal {${GOAL_FIELDS}
		}
		buckets {
			start
			end
			label
			total
			pending
			remaining
			met
			inProgress
		}
		total
		pending
		averagePerPeriod
		currency
	}
}`;

export const GOAL_TRANSACTIONS_QUERY = `
query GoalTransactions($id: UUID!, $startDate: Date!, $endDate: Date!, $page: PageInput) {
	goalTransactions(id: $id, startDate: $startDate, endDate: $endDate, page: $page) {
		items {
${TRANSACTION_ITEM_FIELDS}
		}
		totalCount
		limit
		offset
	}
}`;

export const CREATE_GOAL_MUTATION = `
mutation CreateGoal($input: GoalInput!) {
	createGoal(input: $input) {${GOAL_FIELDS}
	}
}`;

export const UPDATE_GOAL_MUTATION = `
mutation UpdateGoal($id: UUID!, $input: GoalInput!) {
	updateGoal(id: $id, input: $input) {${GOAL_FIELDS}
	}
}`;

export const ARCHIVE_GOAL_MUTATION = `
mutation ArchiveGoal($id: UUID!) {
	archiveGoal(id: $id) {${GOAL_FIELDS}
	}
}`;
