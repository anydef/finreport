import { describe, expect, it } from 'vitest';
import { composeSlug, leafFromInput, validateLeaf } from './categorySlug';

describe('composeSlug', () => {
	it('prepends the parent slug and a dot to the leaf', () => {
		expect(composeSlug('leisure', 'gym')).toBe('leisure.gym');
		expect(composeSlug('leisure.hobbies', 'games')).toBe('leisure.hobbies.games');
	});
	it('leaves a top-level slug as typed (trimmed)', () => {
		expect(composeSlug(null, '  travel ')).toBe('travel');
	});
	it('does not double the prefix when the user pastes the full path', () => {
		expect(composeSlug('leisure', 'leisure.gym')).toBe('leisure.gym');
	});
});

describe('leafFromInput', () => {
	it('strips only a whole-segment parent prefix', () => {
		expect(leafFromInput('leisure', 'leisure.gym')).toBe('gym');
		expect(leafFromInput('leisure', 'leisurex.gym')).toBe('leisurex.gym');
	});
});

describe('validateLeaf', () => {
	it('accepts lowercase letters, digits and underscores', () => {
		expect(validateLeaf('leisure', 'gym_2')).toBeNull();
		expect(validateLeaf(null, 'travel')).toBeNull();
	});
	it('accepts a pasted full path under the matching parent', () => {
		expect(validateLeaf('leisure', 'leisure.gym')).toBeNull();
	});
	it('rejects empty, uppercase, spaces and hyphens', () => {
		expect(validateLeaf('leisure', '  ')).toMatch(/Enter a slug/);
		expect(validateLeaf('leisure', 'Gym')).not.toBeNull();
		expect(validateLeaf('leisure', 'my gym')).not.toBeNull();
		expect(validateLeaf('leisure', 'my-gym')).not.toBeNull();
	});
	it('rejects a dotted leaf that is not the parent path', () => {
		expect(validateLeaf('leisure', 'food.gym')).toMatch(/one segment/);
		expect(validateLeaf('leisure', 'a.b')).toMatch(/one segment/);
	});
	it('rejects any dot at top level', () => {
		expect(validateLeaf(null, 'leisure.gym')).toMatch(/single segment/);
	});
	it('rejects creating under a parent that is already at maximum depth', () => {
		expect(validateLeaf('a.b.c', 'd')).toMatch(/3 levels/);
	});
});
