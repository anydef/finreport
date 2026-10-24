import { describe, expect, it } from 'vitest';
import {
	baselineKey,
	curationRows,
	progressLine,
	readBaseline,
	resolveBaseline,
	rulesLearnedLine,
	writeBaseline,
	type BaselineStorage
} from './curationView';

function fakeStorage(initial: Record<string, string> = {}): BaselineStorage & {
	data: Record<string, string>;
} {
	const data = { ...initial };
	return {
		data,
		getItem: (k) => (k in data ? data[k] : null),
		setItem: (k, v) => {
			data[k] = v;
		}
	};
}

const throwing: BaselineStorage = {
	getItem: () => {
		throw new Error('blocked');
	},
	setItem: () => {
		throw new Error('blocked');
	}
};

describe('progressLine', () => {
	it('says nothing without a baseline', () => {
		expect(progressLine(5, null)).toBeNull();
	});
	it('says nothing when unchanged', () => {
		expect(progressLine(5, 5)).toBeNull();
		expect(progressLine(0, 0)).toBeNull();
	});
	it('reports a fall', () => {
		expect(progressLine(190, 202)).toBe('12 fewer since you opened this page');
	});
	it('uses the same wording for one', () => {
		expect(progressLine(4, 5)).toBe('1 fewer since you opened this page');
	});
	it('reports a rise plainly instead of clamping it', () => {
		expect(progressLine(13, 10)).toBe('3 more since you opened this page');
		expect(progressLine(11, 10)).toBe('1 more since you opened this page');
	});
});

describe('rulesLearnedLine', () => {
	it('omits the line when the figure is unknown', () => {
		expect(rulesLearnedLine(undefined)).toBeNull();
	});
	it('encourages at zero', () => {
		expect(rulesLearnedLine(0)).toBe(
			'No rules learned yet — categorise a repeated merchant and one is learned for you'
		);
	});
	it('is singular for one', () => {
		expect(rulesLearnedLine(1)).toBe('1 rule learned from your decisions');
	});
	it('is plural for many', () => {
		expect(rulesLearnedLine(7)).toBe('7 rules learned from your decisions');
	});
});

describe('baseline storage', () => {
	it('round-trips a baseline, keyed per metric', () => {
		const s = fakeStorage();
		expect(writeBaseline('uncategorized', 12, s)).toBe(true);
		expect(readBaseline('uncategorized', s)).toBe(12);
		expect(readBaseline('needsReview', s)).toBeNull();
		expect(baselineKey('uncategorized')).not.toBe(baselineKey('needsReview'));
	});
	it('treats missing and malformed values as no baseline', () => {
		expect(readBaseline('uncategorized', fakeStorage())).toBeNull();
		for (const bad of ['', 'abc', '-3', '1.5']) {
			expect(
				readBaseline('uncategorized', fakeStorage({ [baselineKey('uncategorized')]: bad }))
			).toBeNull();
		}
	});
	it('keeps a legitimate zero baseline', () => {
		const s = fakeStorage({ [baselineKey('needsReview')]: '0' });
		expect(readBaseline('needsReview', s)).toBe(0);
	});
	it('never throws when storage does', () => {
		expect(readBaseline('uncategorized', throwing)).toBeNull();
		expect(writeBaseline('uncategorized', 3, throwing)).toBe(false);
		expect(writeBaseline('uncategorized', 3, null)).toBe(false);
		expect(readBaseline('uncategorized', null)).toBeNull();
	});
});

describe('resolveBaseline', () => {
	it('stores the current count on first view', () => {
		const s = fakeStorage();
		expect(resolveBaseline('uncategorized', 40, undefined, s)).toBe(40);
		expect(s.data[baselineKey('uncategorized')]).toBe('40');
	});
	it('keeps the stored baseline on later views', () => {
		const s = fakeStorage({ [baselineKey('uncategorized')]: '40' });
		expect(resolveBaseline('uncategorized', 25, undefined, s)).toBe(40);
	});
	it('uses and stores a seed on first view only', () => {
		const s = fakeStorage();
		expect(resolveBaseline('uncategorized', 190, 202, s)).toBe(202);
		expect(resolveBaseline('uncategorized', 150, 999, s)).toBe(202);
	});
	it('yields no baseline when storage throws and nothing is seeded', () => {
		expect(resolveBaseline('uncategorized', 40, undefined, throwing)).toBeNull();
		expect(resolveBaseline('uncategorized', 40, undefined, null)).toBeNull();
	});
	it('still honours a seed when storage throws', () => {
		expect(resolveBaseline('uncategorized', 40, 50, throwing)).toBe(50);
	});
});

describe('curationRows', () => {
	const counts = { uncategorized: 190, needsReview: 196 };
	it('shows counts with deltas', () => {
		const rows = curationRows({ counts, baselines: { uncategorized: 202, needsReview: 196 } });
		expect(rows).toEqual([
			{
				metric: 'uncategorized',
				count: 190,
				label: 'uncategorised',
				delta: '12 fewer since you opened this page'
			},
			{ metric: 'needsReview', count: 196, label: 'need review', delta: null }
		]);
	});
	it('still shows counts when there is no baseline at all (storage unavailable)', () => {
		const rows = curationRows({ counts, baselines: { uncategorized: null, needsReview: null } });
		expect(rows.map((r) => r.delta)).toEqual([null, null]);
		expect(rows).toHaveLength(2);
	});
	it('is empty for a clean queue with no movement', () => {
		expect(
			curationRows({
				counts: { uncategorized: 0, needsReview: 0 },
				baselines: { uncategorized: 0, needsReview: null }
			})
		).toEqual([]);
	});
	it('keeps a metric that just fell to zero', () => {
		const rows = curationRows({
			counts: { uncategorized: 0, needsReview: 0 },
			baselines: { uncategorized: 5, needsReview: 0 }
		});
		expect(rows.map((r) => r.metric)).toEqual(['uncategorized']);
		expect(rows[0].delta).toBe('5 fewer since you opened this page');
	});
	it('shows a rise', () => {
		const rows = curationRows({ counts, baselines: { uncategorized: 180, needsReview: 196 } });
		expect(rows[0].delta).toBe('10 more since you opened this page');
	});
});
