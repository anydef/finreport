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
	}
}`;

export const LOGIN_MUTATION = `
mutation Login($input: LoginInput!) {
	login(input: $input) {
		id
		username
		displayName
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

export const TRANSACTIONS_QUERY = `
query Transactions($filter: TransactionFilter, $page: PageInput) {
	transactions(filter: $filter, page: $page) {
		items {
			id
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
