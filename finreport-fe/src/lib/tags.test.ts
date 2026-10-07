import { describe, expect, it } from 'vitest';
import {
	addTag,
	isValidTag,
	MAX_TAGS_PER_TRANSACTION,
	normalizeTag,
	normalizeTags,
	removeTag
} from './tags';

describe('normalizeTag', () => {
	it('lowercases and trims', () => {
		expect(normalizeTag('  Italy-2026  ')).toBe('italy-2026');
	});

	it('collapses internal whitespace and underscores to a single dash', () => {
		expect(normalizeTag('hobby time')).toBe('hobby-time');
		expect(normalizeTag('hobby_time')).toBe('hobby-time');
		expect(normalizeTag('hobby   time')).toBe('hobby-time');
	});

	it('strips characters outside [a-z0-9-]', () => {
		expect(normalizeTag('café!')).toBe('caf');
		expect(normalizeTag("l'été")).toBe('lt');
	});

	it('collapses repeated dashes and trims leading/trailing dashes', () => {
		expect(normalizeTag('--hobby--time--')).toBe('hobby-time');
		expect(normalizeTag('a---b')).toBe('a-b');
	});

	it('applies NFKC normalization (unicode compatibility forms)', () => {
		// Fullwidth digits/letters normalize to their ASCII equivalents under NFKC.
		expect(normalizeTag('\uFF21\uFF22\uFF23')).toBe('abc');
	});

	it('drops empty-after-normalization tags', () => {
		expect(normalizeTag('   ')).toBe('');
		expect(normalizeTag('---')).toBe('');
		expect(normalizeTag('!!!')).toBe('');
	});

	it('enforces the 1..=32 length bound', () => {
		expect(normalizeTag('a')).toBe('a');
		expect(normalizeTag('a'.repeat(32))).toBe('a'.repeat(32));
		expect(normalizeTag('a'.repeat(33))).toBe('');
	});
});

describe('normalizeTags', () => {
	it('dedupes and sorts', () => {
		expect(normalizeTags(['hobby', 'Hobby', 'italy-2026', 'hobby'])).toEqual([
			'hobby',
			'italy-2026'
		]);
	});

	it('drops empties from the set', () => {
		expect(normalizeTags(['hobby', '   ', '!!!'])).toEqual(['hobby']);
	});

	it('caps at MAX_TAGS_PER_TRANSACTION', () => {
		const many = Array.from({ length: MAX_TAGS_PER_TRANSACTION + 5 }, (_, i) => `tag-${i}`);
		expect(normalizeTags(many)).toHaveLength(MAX_TAGS_PER_TRANSACTION);
	});
});

describe('addTag / removeTag', () => {
	it('adds and normalizes a new tag into an existing sorted set', () => {
		expect(addTag(['hobby'], 'Italy 2026')).toEqual(['hobby', 'italy-2026']);
	});

	it('adding a duplicate is a no-op on the resulting set', () => {
		expect(addTag(['hobby'], 'HOBBY')).toEqual(['hobby']);
	});

	it('removes an exact tag', () => {
		expect(removeTag(['hobby', 'italy-2026'], 'hobby')).toEqual(['italy-2026']);
	});

	it('removing a tag not present is a no-op', () => {
		expect(removeTag(['hobby'], 'nope')).toEqual(['hobby']);
	});
});

describe('isValidTag', () => {
	it('is true for anything that normalizes non-empty', () => {
		expect(isValidTag('hobby')).toBe(true);
	});

	it('is false for whitespace/symbol-only input', () => {
		expect(isValidTag('   ')).toBe(false);
		expect(isValidTag('!!!')).toBe(false);
	});
});
