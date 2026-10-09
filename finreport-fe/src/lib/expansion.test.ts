import { describe, expect, it } from 'vitest';
import { pruneExpanded, toggleExpanded } from './expansion';

describe('toggleExpanded', () => {
	it('opens a row when none is open', () => {
		expect(toggleExpanded(null, 'a')).toBe('a');
	});
	it('collapses the open row when it is clicked again', () => {
		expect(toggleExpanded('a', 'a')).toBeNull();
	});
	it('replaces the open row when another is clicked (one at a time)', () => {
		expect(toggleExpanded('a', 'b')).toBe('b');
	});
});

describe('pruneExpanded', () => {
	it('keeps a row that is still listed', () => {
		expect(pruneExpanded('a', ['a', 'b'])).toBe('a');
	});
	it('collapses when the row left the list', () => {
		expect(pruneExpanded('a', ['b'])).toBeNull();
	});
	it('stays closed when nothing was open', () => {
		expect(pruneExpanded(null, ['a'])).toBeNull();
	});
});
