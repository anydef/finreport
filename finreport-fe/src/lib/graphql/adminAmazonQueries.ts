/**
 * GraphQL documents for `/admin/amazon`: uploading Amazon order-history CSVs
 * and reviewing how the orders line up with bank charges. Uploading records
 * evidence only; it creates and changes no transaction.
 */

const ORDER_FIELDS = `
	orderId
	orderDate
	total
	currency
	remainder
	items {
		asin
		quantity
		title
		lineTotal
		status
		returned
	}`;

const CHARGE_FIELDS = `
	transactionId
	bookingDate
	amount
	currency
	counterpartyName
	description`;

export const PURCHASE_ORDER_MATCHES_QUERY = `
query PurchaseOrderMatches {
	purchaseOrderMatches {
		orderCount
		matches {
			kind
			state
			reasons {
				code
				message
			}
			order {${ORDER_FIELDS}
			}
			charges {${CHARGE_FIELDS}
			}
			refunds {${CHARGE_FIELDS}
			}
		}
		unmatchedOrders {${ORDER_FIELDS}
		}
		unmatchedCharges {${CHARGE_FIELDS}
		}
		subscriptionCharges {${CHARGE_FIELDS}
		}
	}
}`;

export const UPLOAD_PURCHASE_ORDERS_MUTATION = `
mutation UploadPurchaseOrders($files: [String!]!) {
	uploadPurchaseOrders(files: $files) {
		newOrders
		updatedOrders
		unchangedOrders
		files {
			index
			orderCount
			error
		}
	}
}`;
