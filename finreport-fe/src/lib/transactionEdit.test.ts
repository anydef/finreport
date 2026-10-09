import { describe, expect, it } from 'vitest';
import {
	applyCategoryResult,
	applyNoteResult,
	isNoteDirty,
	MAX_NOTE_LENGTH,
	noteProblem,
	noteToSave,
	applyRecurringResult,
	applyTagsResult,
	categoryChangeWarning
} from './transactionEdit';
import { categoryOptionGroups } from './categoryTree';
import type { Category, Transaction } from './graphql/types';

const cat = (id: string, name: string, parentId: string | null, archived = false): Category =>
	({
		id,
		slug: name.toLowerCase(),
		name,
		kind: 'EXPENSE',
		parentId,
		depth: 0,
		archived
	}) as Category;

const base = {
	id: 't1',
	tags: ['a'],
	splits: [{ index: 0, amount: '-1', category: cat('c', 'X', null) }],
	label: {
		category: cat('c', 'X', null),
		source: 'LLM',
		rule: null,
		confidence: 0.4,
		status: 'NEEDS_REVIEW',
		reviewReason: 'LOW_CONFIDENCE',
		proposedCategoryPath: 'x',
		reasoning: 'because'
	},
	recurring: {
		isRecurring: false,
		source: 'AUTO',
		seriesId: null,
		cadence: null,
		medianAmount: null
	}
} as unknown as Transaction;

describe('applyCategoryResult', () => {
	const result = {
		label: { category: cat('f', 'Food', null), source: 'USER', status: 'RESOLVED' }
	} as never;

	it('replaces the label with a resolved user decision and drops stale review metadata', () => {
		const next = applyCategoryResult(base, result);
		expect(next.label?.source).toBe('USER');
		expect(next.label?.status).toBe('RESOLVED');
		expect(next.label?.category?.name).toBe('Food');
		expect(next.label?.confidence).toBeNull();
		expect(next.label?.reviewReason).toBeNull();
		expect(next.label?.reasoning).toBeNull();
	});

	it('clears splits and keeps other fields', () => {
		const next = applyCategoryResult(base, result);
		expect(next.splits).toEqual([]);
		expect(next.tags).toEqual(['a']);
	});
});

describe('applyTagsResult / applyRecurringResult', () => {
	it('replaces the tag set', () => {
		expect(applyTagsResult(base, { tags: ['b', 'c'] }).tags).toEqual(['b', 'c']);
	});
	it('replaces the recurring info', () => {
		const recurring = { ...base.recurring, isRecurring: true, source: 'USER' as const };
		expect(applyRecurringResult(base, { recurring }).recurring.isRecurring).toBe(true);
	});
});

describe('categoryChangeWarning', () => {
	it('is null without splits', () => {
		expect(categoryChangeWarning({ splits: [] })).toBeNull();
	});
	it('names the number of parts when splits exist', () => {
		expect(categoryChangeWarning(base)).toContain('1 parts');
	});
});

describe('categoryOptionGroups', () => {
	it('groups by top-level ancestor with path labels and skips archived', () => {
		const groups = categoryOptionGroups([
			cat('1', 'Food', null),
			cat('2', 'Groceries', '1'),
			cat('3', 'Old', '1', true),
			cat('4', 'Travel', null)
		]);
		expect(groups.map((g) => g.groupLabel)).toEqual(['Food', 'Travel']);
		expect(groups[0].options.map((o) => o.label)).toEqual(['Food', 'Food / Groceries']);
	});
});

describe('notes', () => {
	it('folds the saved note back into the row, null clearing it', () => {
		const tx = { id: 't', note: 'old' } as Transaction;
		expect(applyNoteResult(tx, { note: 'new' }).note).toBe('new');
		expect(applyNoteResult(tx, { note: null }).note).toBeNull();
	});

	it('saves a trimmed note and treats blank as clearing it', () => {
		expect(noteToSave('  paid cash \n')).toBe('paid cash');
		expect(noteToSave('   ')).toBeNull();
		expect(noteToSave('')).toBeNull();
	});

	it('is dirty only when saving would change what is stored', () => {
		expect(isNoteDirty(null, '')).toBe(false);
		expect(isNoteDirty(undefined, '  ')).toBe(false);
		expect(isNoteDirty('paid cash', ' paid cash ')).toBe(false);
		expect(isNoteDirty('paid cash', 'paid card')).toBe(true);
		expect(isNoteDirty('paid cash', '')).toBe(true);
		expect(isNoteDirty(null, 'x')).toBe(true);
	});

	it('flags an over-long note by characters, not bytes', () => {
		expect(noteProblem('ä'.repeat(MAX_NOTE_LENGTH))).toBeNull();
		expect(noteProblem('ä'.repeat(MAX_NOTE_LENGTH + 1))).toMatch(/at most 2000/);
		expect(noteProblem(' '.repeat(MAX_NOTE_LENGTH + 5))).toBeNull();
	});
});
