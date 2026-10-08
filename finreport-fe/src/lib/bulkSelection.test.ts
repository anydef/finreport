import { describe, expect, it } from 'vitest';
import {
	EMPTY_SELECTION,
	describeResult,
	filterKey,
	headerState,
	pruneSelection,
	reviewQueueFilter,
	similarTarget,
	isSelected,
	selectAllMatching,
	selectedCount,
	selectionFilter,
	splitCount,
	toggleHeader,
	toggleRow
} from './bulkSelection';

const plain = { id: 'a', splits: [] };
const split = { id: 'b', splits: [{}] as never };

describe('toggleRow', () => {
	it('adds then removes an id and tracks splits', () => {
		let sel = toggleRow(EMPTY_SELECTION, plain);
		sel = toggleRow(sel, split);
		expect(sel).toEqual({ mode: 'ids', ids: ['a', 'b'], withSplits: ['b'] });
		sel = toggleRow(sel, split);
		expect(sel).toEqual({ mode: 'ids', ids: ['a'], withSplits: [] });
	});
	it('does not mutate its input', () => {
		toggleRow(EMPTY_SELECTION, plain);
		expect(EMPTY_SELECTION).toEqual({ mode: 'ids', ids: [], withSplits: [] });
	});
	it('is a no-op while all-matching', () => {
		const all = selectAllMatching();
		expect(toggleRow(all, plain)).toBe(all);
		expect(isSelected(all, 'zzz')).toBe(true);
	});
});

describe('counts and header', () => {
	it('counts ids exactly and all-matching from totalCount', () => {
		expect(selectedCount(toggleRow(EMPTY_SELECTION, plain), 300)).toBe(1);
		expect(selectedCount(selectAllMatching(), 300)).toBe(300);
	});
	it('header cycles none -> all-matching -> none, and clears a partial selection', () => {
		expect(headerState(EMPTY_SELECTION)).toBe('none');
		const all = toggleHeader(EMPTY_SELECTION);
		expect(headerState(all)).toBe('all');
		expect(toggleHeader(all)).toEqual(EMPTY_SELECTION);
		const some = toggleRow(EMPTY_SELECTION, plain);
		expect(headerState(some)).toBe('some');
		expect(toggleHeader(some)).toEqual(EMPTY_SELECTION);
	});
});

describe('selectionFilter', () => {
	const base = { startDate: '2026-01-01', tags: ['x'] };
	it('adds transactionIds for explicit selections', () => {
		const sel = toggleRow(EMPTY_SELECTION, plain);
		expect(selectionFilter(base, sel)).toEqual({ ...base, transactionIds: ['a'] });
	});
	it('passes the bare filter, without ids, for all-matching', () => {
		const f = selectionFilter(base, selectAllMatching());
		expect(f).toEqual(base);
		expect(f).not.toHaveProperty('transactionIds');
		expect(f).not.toBe(base);
	});
});

describe('splitCount', () => {
	it('is exact for ids and a lower bound for all-matching', () => {
		const sel = toggleRow(EMPTY_SELECTION, split);
		expect(splitCount(sel, [])).toEqual({ count: 1, exact: true });
		expect(splitCount(selectAllMatching(), [split, plain])).toEqual({ count: 1, exact: false });
	});
});

describe('filterKey', () => {
	it('ignores key order, undefined and empty arrays', () => {
		expect(filterKey({ search: 'x', tags: [] })).toBe(
			filterKey({ tags: undefined, search: 'x', categorySlugs: [] })
		);
	});
	it('differs when a condition differs', () => {
		expect(filterKey({ search: 'x' })).not.toBe(filterKey({ search: 'y' }));
	});
});

describe('describeResult', () => {
	const r = (applied: number, failed: number, splitsCleared = 0) => ({
		matched: applied + failed,
		applied,
		failed,
		splitsCleared
	});
	it('reports plain success', () => {
		expect(describeResult(r(3, 0)).tone).toBe('success');
	});
	it('flags a partial failure and mentions retry', () => {
		const o = describeResult(r(4, 1, 2));
		expect(o.tone).toBe('partial');
		expect(o.headline).toContain('1 failed');
		expect(o.details.join(' ')).toMatch(/not atomic/);
		expect(o.details.join(' ')).toContain('2 split definitions');
	});
	it('flags total failure', () => {
		expect(describeResult(r(0, 3)).tone).toBe('failed');
	});
});

describe('pruneSelection', () => {
	it('drops ids that are no longer loaded, keeping the same object when unchanged', () => {
		const sel = { mode: 'ids' as const, ids: ['a', 'b'], withSplits: ['b'] };
		expect(pruneSelection(sel, ['a', 'b', 'c'])).toBe(sel);
		expect(pruneSelection(sel, ['a'])).toEqual({ mode: 'ids', ids: ['a'], withSplits: [] });
	});
	it('leaves all-matching alone', () => {
		const all = selectAllMatching();
		expect(pruneSelection(all, [])).toBe(all);
	});
});

describe('reviewQueueFilter', () => {
	it('always carries needsReview, adding the counterparty when given', () => {
		expect(reviewQueueFilter()).toEqual({ needsReview: true });
		expect(reviewQueueFilter('amazon')).toEqual({
			needsReview: true,
			counterpartyKeys: ['amazon']
		});
	});
	it('keeps a bulk filter inside the queue', () => {
		const f = selectionFilter(reviewQueueFilter('amazon'), {
			mode: 'ids',
			ids: ['a'],
			withSplits: []
		});
		expect(f).toEqual({ needsReview: true, counterpartyKeys: ['amazon'], transactionIds: ['a'] });
		expect(selectionFilter(reviewQueueFilter(), selectAllMatching())).toEqual({
			needsReview: true
		});
	});
});

describe('similarTarget', () => {
	const rows = [
		{ id: 'a', counterpartyKey: 'amazon' },
		{ id: 'b', counterpartyKey: null },
		{ id: 'c' }
	];
	const ids = (...i: string[]) => ({ mode: 'ids' as const, ids: i, withSplits: [] });
	it('is ready for exactly one keyed transaction', () => {
		expect(similarTarget(ids('a'), rows)).toEqual({ kind: 'ready', counterpartyKey: 'amazon' });
	});
	it('reports a missing key rather than offering a dead action', () => {
		expect(similarTarget(ids('b'), rows)).toEqual({ kind: 'no-key' });
		expect(similarTarget(ids('c'), rows)).toEqual({ kind: 'no-key' });
	});
	it('needs exactly one selection', () => {
		expect(similarTarget(EMPTY_SELECTION, rows)).toEqual({ kind: 'needs-one' });
		expect(similarTarget(ids('a', 'b'), rows)).toEqual({ kind: 'needs-one' });
		expect(similarTarget(selectAllMatching(), rows)).toEqual({ kind: 'needs-one' });
	});
});
