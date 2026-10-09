import { describe, expect, it } from 'vitest';
import { moveHighlight, suggestTags } from './tagSuggestions';

const known = [
	{ tag: 'holiday', transactionCount: 12 },
	{ tag: 'holiday-italy', transactionCount: 3 },
	{ tag: 'work-holiday', transactionCount: 5 },
	{ tag: 'hobby', transactionCount: 4 },
	{ tag: 'reimbursable', transactionCount: 1 }
];

const names = (s: ReturnType<typeof suggestTags>) => s.map((x) => x.tag);

describe('suggestTags', () => {
	it('lists the most used tags for an empty draft, without adding a new entry', () => {
		const out = suggestTags(known, '  ', []);
		expect(names(out)).toEqual([
			'holiday',
			'work-holiday',
			'hobby',
			'holiday-italy',
			'reimbursable'
		]);
		expect(out.every((s) => !s.isNew)).toBe(true);
	});

	it('ranks prefix matches before substring matches, each by count', () => {
		expect(names(suggestTags(known, 'holi', []))).toEqual([
			'holiday',
			'holiday-italy',
			'work-holiday',
			'holi'
		]);
	});

	it('carries the usage count and marks the typed remainder as new', () => {
		const out = suggestTags(known, 'holi', []);
		expect(out[0]).toEqual({ tag: 'holiday', count: 12, isNew: false });
		expect(out.at(-1)).toEqual({ tag: 'holi', count: null, isNew: true });
	});

	it('suggests the normalised form of what was typed', () => {
		const out = suggestTags(known, 'Work Holiday', []);
		expect(out[0].tag).toBe('work-holiday');
		expect(out.some((s) => s.isNew)).toBe(false);
		expect(suggestTags([], 'My Trip', [])).toEqual([{ tag: 'my-trip', count: null, isNew: true }]);
	});

	it('does not offer a new entry for an exact existing tag', () => {
		expect(suggestTags(known, 'hobby', [])).toEqual([{ tag: 'hobby', count: 4, isNew: false }]);
	});

	it('leaves out tags already on the transaction and does not re-offer them as new', () => {
		expect(names(suggestTags(known, 'holiday', ['holiday']))).toEqual([
			'holiday-italy',
			'work-holiday'
		]);
	});

	it('returns nothing for a draft that normalises to empty', () => {
		expect(suggestTags(known, '!!', [])).toEqual([]);
	});

	it('stops offering new tags at the per-transaction cap', () => {
		const full = Array.from({ length: 10 }, (_, i) => `t${i}`);
		expect(suggestTags([], 'fresh', full)).toEqual([]);
	});

	it('honours the limit on existing matches', () => {
		expect(names(suggestTags(known, 'h', [], 2))).toEqual(['holiday', 'hobby', 'h']);
	});
});

describe('moveHighlight', () => {
	it('starts at the first item going down and the last going up', () => {
		expect(moveHighlight(-1, 'ArrowDown', 3)).toBe(0);
		expect(moveHighlight(-1, 'ArrowUp', 3)).toBe(2);
	});
	it('wraps at both ends', () => {
		expect(moveHighlight(2, 'ArrowDown', 3)).toBe(0);
		expect(moveHighlight(0, 'ArrowUp', 3)).toBe(2);
	});
	it('stays unhighlighted for an empty list', () => {
		expect(moveHighlight(0, 'ArrowDown', 0)).toBe(-1);
	});
});
