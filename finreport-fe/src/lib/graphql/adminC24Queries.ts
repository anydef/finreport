/**
 * GraphQL documents for `/admin/c24`: uploading C24 CSV account exports.
 * Unlike Amazon orders, C24 rows are transactions, so there is no review
 * step: upload, then report what changed.
 */

export const UPLOAD_C24_TRANSACTIONS_MUTATION = `
mutation UploadC24Transactions($files: [String!]!) {
	uploadC24Transactions(files: $files) {
		files {
			index
			rowCount
			error
		}
		newTransactions
		unchangedTransactions
		updatedTransactions
		newAccounts
		updatedAccounts
		accountCount
		earliestBookingDate
		latestBookingDate
	}
}`;
