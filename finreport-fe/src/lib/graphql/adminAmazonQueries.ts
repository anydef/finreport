/**
 * GraphQL documents for `/admin/amazon`: uploading Amazon order-history CSVs
 * and reviewing how the orders line up with bank charges. Uploading records
 * evidence only; it creates and changes no transaction. Applying a match and
 * undoing it are the only operations here that change transactions.
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
			applications {
				transactionId
				appliedAt
				intact
				parts {
					amount
					categorySlug
					label
				}
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

const OUTCOME_FIELDS = `
	orderId
	outcomes {
		transactionId
		status
		reason
		parts {
			amount
			categorySlug
			label
		}
	}`;

/** Changes transactions: splits each charge of the order into its items. */
export const APPLY_PURCHASE_ORDER_SPLIT_MUTATION = `
mutation ApplyPurchaseOrderSplit($input: ApplyPurchaseOrderInput!) {
	applyPurchaseOrderSplit(input: $input) {${OUTCOME_FIELDS}
	}
}`;

/** Removes the splits this order's application wrote, if still untouched. */
export const UNDO_PURCHASE_ORDER_SPLIT_MUTATION = `
mutation UndoPurchaseOrderSplit($orderId: String!) {
	undoPurchaseOrderSplit(orderId: $orderId) {${OUTCOME_FIELDS}
	}
}`;

/** Suggests a category per item; writes nothing. */
export const PROPOSE_PURCHASE_ORDER_CATEGORIES_MUTATION = `
mutation ProposePurchaseOrderCategories($orderId: String!) {
	proposePurchaseOrderCategories(orderId: $orderId) {
		position
		asin
		title
		categorySlug
		source
		confidence
		reason
	}
}`;
