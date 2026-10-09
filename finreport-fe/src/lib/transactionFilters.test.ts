import { describe, expect, it } from 'vitest';
import categoriesMock from './graphql/mocks/categories.json';
import type { Category } from './graphql/types';
import {
	activeFilters,
	amountWithinRange,
	clearedFilters,
	hasActiveFilters,
	isAmountRangeInverted,
	layerSelection,
	normalizeAmount,
	parsePanelFilters,
	removeFilter,
	toTransactionFilter,
	triFromValue,
	triToValue,
	writePanelFilters
} from './transactionFilters';

const categories = categoriesMock.data.categories as unknown as Category[];
const parse = (qs: string) => parsePanelFilters(new URLSearchParams(qs));

describe('tri-state flags', () => {
	it('maps all three states both ways', () => {
		expect(triToValue(true)).toBe('yes');
		expect(triToValue(false)).toBe('no');
		expect(triToValue(undefined)).toBe('any');
		expect(triFromValue('yes')).toBe(true);
		expect(triFromValue('no')).toBe(false);
		expect(triFromValue('any')).toBeUndefined();
		expect(triFromValue('garbage')).toBeUndefined();
	});

	it('keeps false distinct from absent through the URL', () => {
		const f = parse('recurring=false&transfer=true');
		expect(f.recurring).toBe(false);
		expect(f.transfer).toBe(true);
		expect(f.needsReview).toBeUndefined();
		const qs = writePanelFilters(f).toString();
		expect(qs).toContain('recurring=false');
		expect(qs).not.toContain('needsReview');
	});

	it('only sends flags that are set, false included', () => {
		const filter = toTransactionFilter({ ...clearedFilters(), recurring: false }, categories);
		expect(filter).toEqual({ recurring: false });
		expect('transfer' in filter).toBe(false);
	});
});

describe('normalizeAmount', () => {
	it.each([
		['100', '100'],
		[' 12,5 ', '12.5'],
		['-40', '40'],
		['.5', '0.5'],
		['7.', '7'],
		['', undefined],
		['abc', undefined],
		['1e3', undefined],
		['1.2.3', undefined]
	])('%j -> %j', (raw, expected) => {
		expect(normalizeAmount(raw)).toBe(expected);
	});

	it('treats null and undefined as unbounded', () => {
		expect(normalizeAmount(null)).toBeUndefined();
		expect(normalizeAmount(undefined)).toBeUndefined();
	});
});

describe('amountWithinRange', () => {
	it('bounds the magnitude, inclusively', () => {
		expect(amountWithinRange('-100.00', '100', '100')).toBe(true);
		expect(amountWithinRange('100.00', '100', undefined)).toBe(true);
		expect(amountWithinRange('-99.99', '100', undefined)).toBe(false);
		expect(amountWithinRange('2500.00', undefined, '1000')).toBe(false);
		expect(amountWithinRange('-9.99', null, null)).toBe(true);
	});

	it('flags an inverted range', () => {
		expect(isAmountRangeInverted({ ...clearedFilters(), amountMin: '50', amountMax: '10' })).toBe(
			true
		);
		expect(isAmountRangeInverted({ ...clearedFilters(), amountMin: '10', amountMax: '10' })).toBe(
			false
		);
		expect(isAmountRangeInverted({ ...clearedFilters(), amountMin: '10' })).toBe(false);
	});
});

describe('parse / write round trip', () => {
	it('round-trips every field', () => {
		const qs =
			'accountIds=a,b&categorySlugs=housing&tags=x,y&search=spotify&amountMin=5&amountMax=50.5&recurring=true&transfer=false&needsReview=true&uncategorized=false';
		const f = parse(qs);
		expect(f).toEqual({
			accountIds: ['a', 'b'],
			categorySlugs: ['housing'],
			tags: ['x', 'y'],
			search: 'spotify',
			amountMin: '5',
			amountMax: '50.5',
			recurring: true,
			transfer: false,
			needsReview: true,
			uncategorized: false
		});
		expect(parse(writePanelFilters(f).toString())).toEqual(f);
	});

	it('reads the legacy single accountId, but accountIds wins', () => {
		expect(parse('accountId=z').accountIds).toEqual(['z']);
		expect(parse('accountId=z&accountIds=a').accountIds).toEqual(['a']);
		expect(writePanelFilters(parse('accountId=z')).has('accountId')).toBe(false);
	});

	it('drops garbage amounts and flags instead of throwing', () => {
		const f = parse('amountMin=abc&recurring=maybe');
		expect(f.amountMin).toBeUndefined();
		expect(f.recurring).toBeUndefined();
	});

	it('clears removed filters and leaves foreign params alone', () => {
		const base = new URLSearchParams('preset=last-month&offset=50&search=old&tags=x');
		const next = writePanelFilters(clearedFilters(), base);
		expect(next.toString()).toBe('preset=last-month&offset=50');
		expect(base.get('search')).toBe('old');
	});
});

describe('toTransactionFilter', () => {
	it('is empty for a cleared panel', () => {
		expect(toTransactionFilter(clearedFilters(), categories)).toEqual({});
	});

	it('expands category slugs to descendants and passes the amount bounds', () => {
		const filter = toTransactionFilter(
			{ ...clearedFilters(), categorySlugs: ['housing'], amountMin: '10', amountMax: '99' },
			categories
		);
		expect(filter.categorySlugs).toContain('housing');
		expect(filter.amountMin).toBe('10');
		expect(filter.amountMax).toBe('99');
	});
});

describe('active filters', () => {
	const labels = { accounts: [{ id: 'a', label: 'Giro' }], categories };

	it('is inactive when nothing is set', () => {
		expect(hasActiveFilters(clearedFilters())).toBe(false);
		expect(activeFilters(clearedFilters(), labels)).toEqual([]);
	});

	it('lists one chip per account, tag, flag and the amount range', () => {
		const chips = activeFilters(
			{
				...clearedFilters(),
				accountIds: ['a', 'zzz'],
				tags: ['t'],
				amountMin: '5',
				search: 'x',
				recurring: false,
				needsReview: true
			},
			labels
		).map((c) => c.label);
		expect(chips).toEqual([
			'Account: Giro',
			'Account: zzz',
			'Tag: t',
			'Amount at least 5',
			'Search: "x"',
			'Not recurring',
			'Needs review'
		]);
	});

	it('a false flag counts as active', () => {
		expect(hasActiveFilters({ ...clearedFilters(), transfer: false })).toBe(true);
	});

	it('removes exactly the filter a chip stands for', () => {
		const f = {
			...clearedFilters(),
			accountIds: ['a', 'b'],
			amountMin: '1',
			amountMax: '2',
			recurring: true,
			transfer: true
		};
		expect(removeFilter(f, 'account:a').accountIds).toEqual(['b']);
		const noAmount = removeFilter(f, 'amount');
		expect(noAmount.amountMin).toBeUndefined();
		expect(noAmount.amountMax).toBeUndefined();
		const noFlag = removeFilter(f, 'flag:recurring');
		expect(noFlag.recurring).toBeUndefined();
		expect(noFlag.transfer).toBe(true);
		expect(removeFilter(f, 'flag:accountIds')).toEqual(f);
		expect(removeFilter(f, 'nonsense:x')).toEqual(f);
	});
});

describe('layerSelection', () => {
	it('lets the chart selection win on a shared dimension and keeps the rest', () => {
		const out = layerSelection(
			{ accountIds: ['a', 'b'], search: 'x', recurring: true },
			{ accountIds: ['a'] }
		);
		expect(out).toEqual({ accountIds: ['a'], search: 'x', recurring: true });
	});

	it('ignores unset selection values', () => {
		expect(layerSelection({ search: 'x' }, { accountIds: undefined })).toEqual({ search: 'x' });
	});

	it('drops the panel categories when an Uncategorized node is selected, and vice versa', () => {
		expect(layerSelection({ categorySlugs: ['a'] }, { uncategorized: true })).toEqual({
			uncategorized: true
		});
		expect(layerSelection({ uncategorized: false }, { categorySlugs: ['a'] })).toEqual({
			categorySlugs: ['a']
		});
	});

	it('layers an exact-category selection over the panel without replacing its categories', () => {
		// AND-ed with the panel's own category filter, which stays.
		expect(
			layerSelection({ categorySlugs: ['food', 'food.groceries'] }, { categorySlugsExact: ['food'] })
		).toEqual({ categorySlugs: ['food', 'food.groceries'], categorySlugsExact: ['food'] });
		// An exact category contradicts a panel "Uncategorized".
		expect(layerSelection({ uncategorized: true }, { categorySlugsExact: ['food'] })).toEqual({
			categorySlugsExact: ['food']
		});
	});

	it('lets a needs-review selection (true or false) override the panel flag', () => {
		expect(layerSelection({ needsReview: true }, { needsReview: false })).toEqual({
			needsReview: false
		});
		expect(layerSelection({ search: 'x' }, { uncategorized: true, needsReview: false })).toEqual({
			search: 'x',
			uncategorized: true,
			needsReview: false
		});
	});
});
