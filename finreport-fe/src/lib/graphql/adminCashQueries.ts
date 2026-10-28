/**
 * GraphQL documents for `/admin/cash`: hand-kept cash accounts.
 *
 * Reading reuses `ACCOUNTS_QUERY` (cash accounts are `source == "cash"`) and
 * `TRANSACTIONS_QUERY` filtered by `accountIds`; only the six mutations live
 * here. Each returns just what the page needs: it refetches the list after.
 */

const CASH_ACCOUNT_FIELDS = `
		id
		source
		label
		displayName
		currency`;

const CASH_TRANSACTION_FIELDS = `
		id
		accountId
		bookingDate
		amount
		currency
		counterpartyName
		description`;

export const CREATE_CASH_ACCOUNT_MUTATION = `
mutation CreateCashAccount($label: String!, $currency: String!) {
	createCashAccount(label: $label, currency: $currency) {${CASH_ACCOUNT_FIELDS}
	}
}`;

export const RENAME_CASH_ACCOUNT_MUTATION = `
mutation RenameCashAccount($accountId: UUID!, $label: String!) {
	renameCashAccount(accountId: $accountId, label: $label) {${CASH_ACCOUNT_FIELDS}
	}
}`;

export const DELETE_CASH_ACCOUNT_MUTATION = `
mutation DeleteCashAccount($accountId: UUID!) {
	deleteCashAccount(accountId: $accountId)
}`;

export const CREATE_CASH_TRANSACTION_MUTATION = `
mutation CreateCashTransaction($accountId: UUID!, $input: CashTransactionInput!) {
	createCashTransaction(accountId: $accountId, input: $input) {${CASH_TRANSACTION_FIELDS}
	}
}`;

export const UPDATE_CASH_TRANSACTION_MUTATION = `
mutation UpdateCashTransaction($transactionId: UUID!, $input: CashTransactionInput!) {
	updateCashTransaction(transactionId: $transactionId, input: $input) {${CASH_TRANSACTION_FIELDS}
	}
}`;

export const DELETE_CASH_TRANSACTION_MUTATION = `
mutation DeleteCashTransaction($transactionId: UUID!) {
	deleteCashTransaction(transactionId: $transactionId)
}`;
