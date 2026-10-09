import { describe, expect, it } from 'vitest';
import {
	defaultKind,
	leafFromName,
	parentCandidates,
	planCategory,
	withCategory
} from './categoryCreate';
import type { Category } from './graphql/types';

const cat = (slug: string, over: Partial<Category> = {}): Category => ({
	id: slug,
	slug,
	name: slug,
	kind: 'EXPENSE',
	parentId: null,
	depth: slug.split('.').length,
	archived: false,
	origin: 'seed',
	...over
});

describe('leafFromName', () => {
	it('lowercases and joins words with underscores', () => {
		expect(leafFromName('Board Games!')).toBe('board_games');
		expect(leafFromName('  Café  ')).toBe('cafe');
	});
	it('is empty for a name with no usable characters', () => {
		expect(leafFromName('???')).toBe('');
	});
});

describe('planCategory', () => {
	it('composes a child slug from the parent plus one segment', () => {
		const plan = planCategory({
			parentSlug: 'personal',
			leaf: 'hobby',
			name: ' Hobby ',
			kind: 'EXPENSE'
		});
		expect(plan).toEqual({
			ok: true,
			slug: 'personal.hobby',
			input: { slug: 'personal.hobby', name: 'Hobby', kind: 'EXPENSE', parentSlug: 'personal' }
		});
	});
	it('strips a pasted parent prefix rather than doubling it', () => {
		const plan = planCategory({
			parentSlug: 'personal',
			leaf: 'personal.hobby',
			name: 'H',
			kind: 'EXPENSE'
		});
		expect(plan).toMatchObject({ ok: true, slug: 'personal.hobby' });
	});
	it('rejects a dotted top-level slug', () => {
		const plan = planCategory({ parentSlug: null, leaf: 'a.b', name: 'AB', kind: 'EXPENSE' });
		expect(plan.ok).toBe(false);
	});
	it('rejects an empty name and an invalid segment', () => {
		expect(planCategory({ parentSlug: null, leaf: 'ok', name: ' ', kind: 'EXPENSE' })).toEqual({
			ok: false,
			problem: 'Enter a name.'
		});
		expect(planCategory({ parentSlug: null, leaf: 'No Good', name: 'x', kind: 'EXPENSE' }).ok).toBe(
			false
		);
	});
});

describe('parentCandidates / defaultKind / withCategory', () => {
	const list = [
		cat('b'),
		cat('a', { kind: 'INCOME' }),
		cat('a.x.y'),
		cat('old', { archived: true })
	];
	it('lists active categories shallower than the depth limit, sorted', () => {
		expect(parentCandidates(list).map((c) => c.slug)).toEqual(['a', 'b']);
	});
	it('inherits the parent kind, else expense', () => {
		expect(defaultKind('a', list)).toBe('INCOME');
		expect(defaultKind(null, list)).toBe('EXPENSE');
	});
	it('replaces an existing slug instead of duplicating it', () => {
		const out = withCategory(list, cat('a', { name: 'Renamed' }));
		expect(out.filter((c) => c.slug === 'a')).toHaveLength(1);
		expect(withCategory(list, cat('new')).map((c) => c.slug)).toContain('new');
	});
});
