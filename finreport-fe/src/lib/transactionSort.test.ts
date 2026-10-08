import { describe, expect, it } from 'vitest';
import type { TransactionSort } from './graphql/types';
import {
	ariaSort,
	compareBySort,
	DEFAULT_SORT,
	nextSort,
	parseSort,
	sortKey,
	sortVariable,
	writeSort
} from './transactionSort';

const parse = (qs: string) => parseSort(new URLSearchParams(qs));
const sort = (field: TransactionSort['field'], direction: TransactionSort['direction']) => ({
	field,
	direction
});

describe('parseSort', () => {
	it('defaults to newest booking date first when absent or unknown', () => {
		expect(parse('')).toEqual(DEFAULT_SORT);
		expect(parse('sort=bogus&dir=asc')).toEqual(DEFAULT_SORT);
		expect(parse('dir=asc')).toEqual(DEFAULT_SORT);
	});

	it('reads field and direction, case-insensitively', () => {
		expect(parse('sort=amount&dir=asc')).toEqual(sort('AMOUNT', 'ASC'));
		expect(parse('sort=COUNTERPARTY&dir=DESC')).toEqual(sort('COUNTERPARTY_NAME', 'DESC'));
		expect(parse('sort=category&dir=asc')).toEqual(sort('CATEGORY', 'ASC'));
	});

	it("falls back to the column's first direction for a missing or invalid dir", () => {
		expect(parse('sort=amount')).toEqual(sort('AMOUNT', 'DESC'));
		expect(parse('sort=counterparty&dir=sideways')).toEqual(sort('COUNTERPARTY_NAME', 'ASC'));
	});
});

describe('writeSort', () => {
	it('writes nothing for the default, and removes a previous sort', () => {
		expect(writeSort(DEFAULT_SORT).toString()).toBe('');
		expect(writeSort(DEFAULT_SORT, new URLSearchParams('sort=amount&dir=asc')).toString()).toBe('');
	});

	it('round-trips through parseSort', () => {
		for (const field of ['BOOKING_DATE', 'AMOUNT', 'COUNTERPARTY_NAME', 'CATEGORY'] as const) {
			for (const direction of ['ASC', 'DESC'] as const) {
				const s = sort(field, direction);
				expect(parseSort(writeSort(s))).toEqual(s);
			}
		}
	});

	it('resets paging but keeps unrelated params', () => {
		const next = writeSort(
			sort('AMOUNT', 'DESC'),
			new URLSearchParams('offset=150&search=rewe&preset=last-month')
		);
		expect(next.has('offset')).toBe(false);
		expect(next.get('search')).toBe('rewe');
		expect(next.get('preset')).toBe('last-month');
		expect(next.get('sort')).toBe('amount');
	});

	it('does not mutate its input', () => {
		const input = new URLSearchParams('offset=50');
		writeSort(sort('AMOUNT', 'ASC'), input);
		expect(input.toString()).toBe('offset=50');
	});
});

describe('nextSort', () => {
	it('starts a new column in its natural direction', () => {
		expect(nextSort(DEFAULT_SORT, 'AMOUNT')).toEqual(sort('AMOUNT', 'DESC'));
		expect(nextSort(DEFAULT_SORT, 'COUNTERPARTY_NAME')).toEqual(sort('COUNTERPARTY_NAME', 'ASC'));
		expect(nextSort(sort('AMOUNT', 'ASC'), 'CATEGORY')).toEqual(sort('CATEGORY', 'ASC'));
	});

	it('reverses the active column, including the default date sort', () => {
		expect(nextSort(DEFAULT_SORT, 'BOOKING_DATE')).toEqual(sort('BOOKING_DATE', 'ASC'));
		expect(nextSort(sort('AMOUNT', 'DESC'), 'AMOUNT')).toEqual(sort('AMOUNT', 'ASC'));
		expect(nextSort(sort('AMOUNT', 'ASC'), 'AMOUNT')).toEqual(sort('AMOUNT', 'DESC'));
	});
});

describe('ariaSort', () => {
	it('marks only the active column', () => {
		const current = sort('AMOUNT', 'ASC');
		expect(ariaSort(current, 'AMOUNT')).toBe('ascending');
		expect(ariaSort(sort('AMOUNT', 'DESC'), 'AMOUNT')).toBe('descending');
		expect(ariaSort(current, 'CATEGORY')).toBe('none');
		expect(ariaSort(DEFAULT_SORT, 'BOOKING_DATE')).toBe('descending');
	});
});

describe('sortVariable / sortKey', () => {
	it('omits the default so an unsorted view sends what it always sent', () => {
		expect(sortVariable(DEFAULT_SORT)).toBeUndefined();
		expect(sortVariable(sort('AMOUNT', 'DESC'))).toEqual(sort('AMOUNT', 'DESC'));
	});

	it('gives different sorts different keys (a selection is scoped to one)', () => {
		expect(sortKey(sort('AMOUNT', 'ASC'))).not.toBe(sortKey(sort('AMOUNT', 'DESC')));
		expect(sortKey(sort('AMOUNT', 'ASC'))).not.toBe(sortKey(sort('CATEGORY', 'ASC')));
	});
});

describe('compareBySort (the server ordering, as the mock applies it)', () => {
	const row = (
		id: string,
		over: Partial<{
			bookingDate: string;
			externalId: string;
			amount: string;
			counterpartyName: string | null;
			category: string | null;
			split: boolean;
		}> = {}
	) => ({
		id,
		bookingDate: over.bookingDate ?? '2026-01-01',
		externalId: over.externalId ?? id,
		amount: over.amount ?? '1',
		counterpartyName: over.counterpartyName === undefined ? 'x' : over.counterpartyName,
		label: over.category ? ({ category: { name: over.category } } as never) : null,
		splits: over.split ? ([{}] as never) : []
	});
	const order = (rows: ReturnType<typeof row>[], s: TransactionSort) =>
		[...rows].sort(compareBySort(s)).map((r) => r.id);

	it('sorts the signed amount', () => {
		const rows = [
			row('out', { amount: '-500' }),
			row('in', { amount: '20' }),
			row('small', { amount: '-5' })
		];
		expect(order(rows, sort('AMOUNT', 'DESC'))).toEqual(['in', 'small', 'out']);
		expect(order(rows, sort('AMOUNT', 'ASC'))).toEqual(['out', 'small', 'in']);
	});

	it('sorts names case-insensitively with missing values last in both directions', () => {
		const rows = [
			row('b', { counterpartyName: 'bravo' }),
			row('none', { counterpartyName: null }),
			row('a', { counterpartyName: 'Alpha' })
		];
		expect(order(rows, sort('COUNTERPARTY_NAME', 'ASC'))).toEqual(['a', 'b', 'none']);
		expect(order(rows, sort('COUNTERPARTY_NAME', 'DESC'))).toEqual(['b', 'a', 'none']);
	});

	it('puts uncategorised and split rows last in both directions', () => {
		const rows = [
			row('unlabelled'),
			row('z', { category: 'Zucchini' }),
			row('split', { category: 'Apples', split: true }),
			row('a', { category: 'apples' })
		];
		expect(order(rows, sort('CATEGORY', 'ASC')).slice(0, 2)).toEqual(['a', 'z']);
		expect(order(rows, sort('CATEGORY', 'DESC')).slice(0, 2)).toEqual(['z', 'a']);
		expect(order(rows, sort('CATEGORY', 'DESC')).slice(2).sort()).toEqual(['split', 'unlabelled']);
	});

	it('breaks ties on id so equal keys have exactly one order', () => {
		const rows = [row('c'), row('a'), row('b')];
		const forward = order(rows, sort('AMOUNT', 'DESC'));
		expect(order([...rows].reverse(), sort('AMOUNT', 'DESC'))).toEqual(forward);
		expect(new Set(forward).size).toBe(3);
	});

	it('orders by date with the default matching newest first', () => {
		const rows = [
			row('old', { bookingDate: '2026-01-01' }),
			row('new', { bookingDate: '2026-03-01' })
		];
		expect(order(rows, DEFAULT_SORT)).toEqual(['new', 'old']);
		expect(order(rows, sort('BOOKING_DATE', 'ASC'))).toEqual(['old', 'new']);
	});
});
