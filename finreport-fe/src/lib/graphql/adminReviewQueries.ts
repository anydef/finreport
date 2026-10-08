/**
 * Admin-review-owned barrel over the frozen `queries.ts` (WP0-owned; §10
 * shared-file protocol). The review queue and split editor only need a
 * subset of the iteration-2 operations — re-exporting here keeps
 * `routes/(app)/review/**` and the review-queue components importing
 * from a file this package owns, without duplicating (and risking drift
 * from) the operation strings themselves.
 */
export {
	CATEGORIES_QUERY,
	SET_TRANSACTION_CATEGORY_MUTATION,
	CLEAR_TRANSACTION_CATEGORY_MUTATION,
	SPLIT_TRANSACTION_MUTATION,
	UNSPLIT_TRANSACTION_MUTATION,
	SET_RULE_STATE_MUTATION,
	CREATE_CATEGORY_MUTATION,
	ENSURE_CATEGORY_MUTATION
} from './queries';

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

/**
 * Own variant of `REVIEW_QUEUE_QUERY` (same `reviewQueue` operation as the
 * frozen one in `queries.ts`, so mock dispatch — keyed on operation name —
 * still resolves it) that additionally selects `splits`, which the split
 * editor needs to prefill a `split_mismatch` item's existing parts and to
 * know whether *unsplit* applies.
 */
export const REVIEW_QUEUE_WITH_SPLITS_QUERY = `
query ReviewQueue($page: PageInput) {
	reviewQueue(page: $page) {
		transactions {
			id
			accountId
			bookingDate
			amount
			currency
			counterpartyName
			counterpartyKey
			description
			label {
				source
				status
				reviewReason
				proposedCategoryPath
				reasoning
				confidence
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
		}
		pendingRules {${RULE_FIELDS}
		}
		totalCount
	}
}`;

/**
 * Held transactions narrowed to one counterparty ("find similar"). The
 * `reviewQueue` operation takes no filter, so this uses `transactions` with
 * `needsReview: true`; the selection matches `REVIEW_QUEUE_WITH_SPLITS_QUERY`
 * so `ReviewCard` renders either source.
 */
export const REVIEW_HELD_TRANSACTIONS_QUERY = `
query ReviewHeldTransactions($filter: TransactionFilter, $page: PageInput) {
	transactions(filter: $filter, page: $page) {
		items {
			id
			accountId
			bookingDate
			amount
			currency
			counterpartyName
			counterpartyKey
			description
			label {
				source
				status
				reviewReason
				proposedCategoryPath
				reasoning
				confidence
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
		}
		totalCount
	}
}`;
