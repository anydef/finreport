/**
 * Sorting for the transaction tables, kept out of the Svelte components so it
 * is testable: reading the sort out of search params, writing it back, what a
 * header click does next, and the `aria-sort` value of a column.
 *
 * The sort is applied by the server (`transactions(sort:)`,
 * `goalTransactions(sort:)`): the lists are paged, so sorting only the loaded
 * rows would present a reordered page as "sorted by amount". Search params:
 * `sort` (`date | amount | counterparty | category`) and `dir` (`asc | desc`).
 * Absent means the default, newest booking date first, and is never sent.
 */
import type {
	SortDirection,
	Transaction,
	TransactionSort,
	TransactionSortField
} from '$lib/graphql/types';

export const SORT_PARAMS = ['sort', 'dir'] as const;

export const DEFAULT_SORT: TransactionSort = { field: 'BOOKING_DATE', direction: 'DESC' };

const FIELD_BY_PARAM: Record<string, TransactionSortField> = {
	date: 'BOOKING_DATE',
	amount: 'AMOUNT',
	counterparty: 'COUNTERPARTY_NAME',
	category: 'CATEGORY'
};
const PARAM_BY_FIELD = Object.fromEntries(
	Object.entries(FIELD_BY_PARAM).map(([param, field]) => [field, param])
) as Record<TransactionSortField, string>;

/**
 * The direction a column starts in when first chosen: dates and amounts
 * newest/largest first, text columns A to Z.
 */
const FIRST_DIRECTION: Record<TransactionSortField, SortDirection> = {
	BOOKING_DATE: 'DESC',
	AMOUNT: 'DESC',
	COUNTERPARTY_NAME: 'ASC',
	CATEGORY: 'ASC'
};

export const sameSort = (a: TransactionSort, b: TransactionSort): boolean =>
	a.field === b.field && a.direction === b.direction;

/**
 * Read the sort out of search params. Anything unknown falls back to the
 * default rather than erroring, so a hand-edited URL still loads. A missing
 * `dir` means the column's first direction.
 */
export function parseSort(params: URLSearchParams): TransactionSort {
	const field = FIELD_BY_PARAM[params.get('sort')?.toLowerCase() ?? ''];
	if (!field) return DEFAULT_SORT;
	const dir = params.get('dir')?.toLowerCase();
	const direction: SortDirection =
		dir === 'asc' ? 'ASC' : dir === 'desc' ? 'DESC' : FIRST_DIRECTION[field];
	return { field, direction };
}

/**
 * Write the sort into a copy of `params`. The default is written as nothing,
 * so an unsorted URL stays clean. Paging (`offset`) is dropped: page 4 of a
 * newly reordered list is an arbitrary slice.
 */
export function writeSort(
	sort: TransactionSort,
	params: URLSearchParams = new URLSearchParams()
): URLSearchParams {
	const next = new URLSearchParams(params);
	for (const key of SORT_PARAMS) next.delete(key);
	next.delete('offset');
	if (!sameSort(sort, DEFAULT_SORT)) {
		next.set('sort', PARAM_BY_FIELD[sort.field]);
		next.set('dir', sort.direction.toLowerCase());
	}
	return next;
}

/** What a click on `field`'s header does: reverse the active column, else start the new one. */
export function nextSort(current: TransactionSort, field: TransactionSortField): TransactionSort {
	if (current.field === field) {
		return { field, direction: current.direction === 'ASC' ? 'DESC' : 'ASC' };
	}
	return { field, direction: FIRST_DIRECTION[field] };
}

export type AriaSort = 'ascending' | 'descending' | 'none';

export function ariaSort(current: TransactionSort, field: TransactionSortField): AriaSort {
	if (current.field !== field) return 'none';
	return current.direction === 'ASC' ? 'ascending' : 'descending';
}

/**
 * The `sort` query variable: `undefined` for the default, so a default view
 * sends exactly what it sent before sorting existed.
 */
export function sortVariable(sort: TransactionSort): TransactionSort | undefined {
	return sameSort(sort, DEFAULT_SORT) ? undefined : sort;
}

/** Stable identity of a sort; a bulk selection is only valid for the sort it was made under. */
export function sortKey(sort: TransactionSort): string {
	return `${sort.field}:${sort.direction}`;
}

type Sortable = Pick<
	Transaction,
	'id' | 'bookingDate' | 'externalId' | 'amount' | 'counterpartyName' | 'label' | 'splits'
>;

const text = (value: string | null | undefined) => value?.toLowerCase() ?? null;

function compareValues(a: string | number | null, b: string | number | null): number {
	if (a === b) return 0;
	return a! < b! ? -1 : 1;
}

/**
 * The server's ordering, for the mock backend (and as an executable spec of
 * it): the key in the requested direction with missing keys last in both
 * directions, then newest booking date, then id as the tiebreaker.
 * `AMOUNT` is the signed value; `CATEGORY` is the label's category name, and
 * a split transaction has no single category.
 */
export function compareBySort(sort: TransactionSort) {
	const sign = sort.direction === 'ASC' ? 1 : -1;
	const key = (tx: Sortable): string | number | null => {
		switch (sort.field) {
			case 'BOOKING_DATE':
				return tx.bookingDate;
			case 'AMOUNT':
				return Number(tx.amount);
			case 'COUNTERPARTY_NAME':
				return text(tx.counterpartyName);
			case 'CATEGORY':
				return (tx.splits?.length ?? 0) > 0 ? null : text(tx.label?.category?.name);
		}
	};
	return (a: Sortable, b: Sortable): number => {
		const ka = key(a);
		const kb = key(b);
		if (ka === null && kb !== null) return 1;
		if (kb === null && ka !== null) return -1;
		const primary = ka === null ? 0 : sign * compareValues(ka, kb);
		if (primary) return primary;
		if (sort.field === 'BOOKING_DATE') {
			// Date ties break on the external id in the same direction, then id.
			return sign * (compareValues(a.externalId, b.externalId) || compareValues(a.id, b.id));
		}
		return -compareValues(a.bookingDate, b.bookingDate) || -compareValues(a.id, b.id);
	};
}
