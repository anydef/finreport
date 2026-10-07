import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import {
	averageOverCompleted,
	bucketRangeParams,
	bucketTone,
	currentBucket,
	describeGoalPeriod,
	emptyGoalForm,
	formatMoney,
	formToInput,
	goalToForm,
	goalTotals,
	isMet,
	progressFraction,
	progressRange,
	remainingAmount,
	selectedBucket,
	toneBarClass,
	toneText,
	toneTextClass,
	shapeCumulativeSeries,
	shapeGoalBars,
	validateGoalForm
} from './goalsView';
import type { Goal, GoalBucket, GoalProgress, Transaction } from './graphql/types';

function loadMock<T>(name: string): T {
	const file = path.resolve(__dirname, 'graphql/mocks', name);
	return JSON.parse(readFileSync(file, 'utf-8')).data as T;
}

const recurring = loadMock<{ goalProgress: GoalProgress }>(
	'goal-progress-recurring.json'
).goalProgress;
const fixed = loadMock<{ goalProgress: GoalProgress }>('goal-progress-fixed.json').goalProgress;
const goals = loadMock<{ goals: Goal[] }>('goals.json').goals;

function bucket(over: Partial<GoalBucket>): GoalBucket {
	return {
		start: '2026-01-01',
		end: '2026-01-31',
		label: 'Jan 2026',
		total: '0.0000',
		pending: '0.0000',
		remaining: '0.0000',
		met: true,
		inProgress: false,
		...over
	};
}

function tx(over: Partial<Transaction>): Transaction {
	return {
		id: 't',
		accountId: 'a',
		source: 's',
		externalId: 'e',
		bookingDate: '2026-02-10',
		valutaDate: null,
		bookingStatus: 'BOOKED',
		amount: '0.00',
		currency: 'EUR',
		counterpartyName: null,
		counterpartyIban: null,
		description: null,
		transactionType: null,
		label: null,
		splits: [],
		tags: [],
		transfer: null,
		recurring: { isRecurring: false } as Transaction['recurring'],
		...over
	};
}

const saving = { id: 'c', slug: 'savings.x', name: 'X', kind: 'SAVING' } as never;
const expense = { id: 'd', slug: 'food', name: 'Food', kind: 'EXPENSE' } as never;

describe('formatMoney', () => {
	it('rounds to cents and groups thousands', () => {
		expect(formatMoney('1850.0000', 'EUR')).toBe('1,850.00 EUR');
		expect(formatMoney('183.8909', 'EUR')).toBe('183.89 EUR');
		expect(formatMoney('0.005', 'EUR')).toBe('0.01 EUR');
	});

	it('keeps a minus sign for negatives', () => {
		expect(formatMoney('-12.9000', 'EUR')).toBe('−12.90 EUR');
	});
});

describe('remainingAmount / isMet', () => {
	it('is amount minus total, negative when over', () => {
		expect(remainingAmount('200.0000', '168.4000')).toBe('31.60');
		expect(remainingAmount('200.0000', '212.9000')).toBe('-12.90');
		expect(remainingAmount('0.3', '0.1')).toBe('0.20');
	});

	it('met is <= for a limit and >= for a target', () => {
		expect(isMet('SPENDING_LIMIT', '200.0000', '200.0000')).toBe(true);
		expect(isMet('SPENDING_LIMIT', '200.0000', '200.0001')).toBe(false);
		expect(isMet('SAVING_TARGET', '3000.0000', '2999.9999')).toBe(false);
		expect(isMet('SAVING_TARGET', '3000.0000', '3000.0000')).toBe(true);
	});

	it('agrees with every recurring fixture bucket', () => {
		for (const b of recurring.buckets) {
			expect(formatMoney(remainingAmount(recurring.goal.amount, b.total), 'EUR')).toBe(
				formatMoney(b.remaining, 'EUR')
			);
			expect(isMet(recurring.goal.type, recurring.goal.amount, b.total)).toBe(b.met);
		}
	});
});

describe('progressFraction', () => {
	it('is the exact ratio, clamped to 0..1', () => {
		expect(progressFraction('50', '200')).toBe(0.25);
		expect(progressFraction('142.5000', '200.0000')).toBe(0.7125);
		expect(progressFraction('250', '200')).toBe(1);
		expect(progressFraction('0', '200')).toBe(0);
		expect(progressFraction('-5', '200')).toBe(0);
	});

	it('guards a zero threshold', () => {
		expect(progressFraction('10', '0')).toBe(0);
	});
});

describe('bucketTone', () => {
	const limit = (b: Partial<GoalBucket>) => bucketTone('SPENDING_LIMIT', '200.0000', bucket(b));
	const target = (b: Partial<GoalBucket>) => bucketTone('SAVING_TARGET', '200.0000', bucket(b));

	it('completed buckets are good or bad', () => {
		expect(limit({ total: '199.99' })).toBe('good');
		expect(limit({ total: '200.01' })).toBe('bad');
		expect(target({ total: '200' })).toBe('good');
		expect(target({ total: '100' })).toBe('bad');
	});

	it('an in-progress target short of the mark is not a failure', () => {
		expect(target({ total: '100', inProgress: true })).toBe('neutral');
		expect(target({ total: '200', inProgress: true })).toBe('good');
	});

	it('an in-progress limit already blown is bad', () => {
		expect(limit({ total: '250', inProgress: true })).toBe('bad');
		expect(limit({ total: '100', inProgress: true })).toBe('good');
	});
});

describe('currentBucket / progressRange', () => {
	it('prefers the in-progress bucket', () => {
		expect(currentBucket(recurring.buckets)?.label).toBe('Oct 2026');
	});

	it('falls back to the latest bucket, and null for none', () => {
		const done = [bucket({ label: 'A' }), bucket({ label: 'B' })];
		expect(currentBucket(done)?.label).toBe('B');
		expect(currentBucket([])).toBeNull();
	});

	it('spans first start to last end', () => {
		expect(progressRange(recurring.buckets)).toEqual({ start: '2025-11-01', end: '2026-10-31' });
		expect(progressRange([])).toBeNull();
	});
});

describe('averageOverCompleted', () => {
	it('excludes the in-progress bucket and matches the fixture', () => {
		expect(averageOverCompleted(recurring.buckets)).toBe('183.89');
		expect(formatMoney(recurring.averagePerPeriod, 'EUR')).toBe(
			formatMoney(averageOverCompleted(recurring.buckets), 'EUR')
		);
	});

	it('is exact on a tie, rounding half away from zero', () => {
		expect(averageOverCompleted([bucket({ total: '0.01' }), bucket({ total: '0.00' })])).toBe(
			'0.01'
		);
	});

	it('is 0.00 with no completed bucket', () => {
		expect(averageOverCompleted([])).toBe('0.00');
		expect(averageOverCompleted([bucket({ inProgress: true, total: '99' })])).toBe('0.00');
	});
});

describe('shapeGoalBars', () => {
	it('keeps order, ranges and tone', () => {
		const bars = shapeGoalBars(recurring.goal, recurring.buckets);
		expect(bars).toHaveLength(12);
		expect(bars[0]).toMatchObject({
			label: 'Nov 2025',
			start: '2025-11-01',
			end: '2025-11-30',
			total: 168.4,
			tone: 'good'
		});
		expect(bars[1].tone).toBe('bad');
		expect(bars[11]).toMatchObject({ inProgress: true, pending: 18 });
	});
});

describe('goalTotals', () => {
	it('recurring: current-period remaining, completed-only average, pending kept apart', () => {
		const t = goalTotals(recurring);
		expect(t).toEqual({
			totalLabel: 'Spent',
			total: '2165.3000',
			remainingLabel: 'Remaining this period',
			remaining: '57.5000',
			average: '183.89',
			pending: '42.9900'
		});
	});

	it('fixed: saved vs target, no average', () => {
		const t = goalTotals(fixed);
		expect(t.totalLabel).toBe('Saved');
		expect(t.remainingLabel).toBe('Remaining');
		expect(t.remaining).toBe('1150.0000');
		expect(t.average).toBeNull();
		expect(t.pending).toBe('150.0000');
	});

	it('pending is never added to the total', () => {
		const t = goalTotals(fixed);
		expect(t.total).toBe('1850.0000');
	});
});

describe('describeGoalPeriod', () => {
	it('describes both kinds', () => {
		expect(describeGoalPeriod(goals[0])).toBe('per month');
		expect(describeGoalPeriod(goals[1])).toBe('2026-01-01 to 2026-12-31');
		expect(describeGoalPeriod({ ...goals[1], endDate: null })).toBe('2026-01-01 to open-ended');
	});
});

describe('bucket drill-down', () => {
	it('maps a bucket to txStart/txEnd', () => {
		expect(bucketRangeParams(recurring.buckets[1])).toEqual({
			txStart: '2025-12-01',
			txEnd: '2025-12-31'
		});
	});

	it('finds the bucket matching the narrowed range only when both ends match', () => {
		expect(selectedBucket(recurring.buckets, '2025-12-01', '2025-12-31')?.label).toBe('Dec 2025');
		expect(selectedBucket(recurring.buckets, '2025-12-01', '2025-12-30')).toBeNull();
		expect(selectedBucket(recurring.buckets, null, null)).toBeNull();
	});
});

describe('shapeCumulativeSeries', () => {
	const limit = recurring.goal;
	const target = fixed.goal;

	it('accumulates spending as positive magnitude, refunds reduce it', () => {
		const series = shapeCumulativeSeries(
			limit,
			[
				tx({ bookingDate: '2026-02-03', amount: '-40.00' }),
				tx({ bookingDate: '2026-02-03', amount: '-10.50' }),
				tx({ bookingDate: '2026-02-20', amount: '15.00' })
			],
			'2026-02-01',
			'2026-02-28'
		);
		expect(series).toEqual([
			{ date: '2026-02-01', total: 0, budget: 200 },
			{ date: '2026-02-03', total: 50.5, budget: 200 },
			{ date: '2026-02-20', total: 35.5, budget: 200 }
		]);
	});

	it('counts only saving-kind parts for a target, as magnitude', () => {
		const series = shapeCumulativeSeries(
			target,
			[
				tx({ bookingDate: '2026-03-01', amount: '-500.00', label: { category: saving } as never }),
				tx({ bookingDate: '2026-03-02', amount: '-80.00', label: { category: expense } as never }),
				tx({
					bookingDate: '2026-03-03',
					amount: '-100.00',
					splits: [
						{ index: 0, amount: '-60.00', category: saving },
						{ index: 1, amount: '-40.00', category: expense }
					]
				})
			],
			'2026-01-01',
			'2026-12-31'
		);
		expect(series.map((p) => p.total)).toEqual([0, 500, 500, 560]);
	});

	it('ignores held-for-review rows and rows outside the range', () => {
		const series = shapeCumulativeSeries(
			limit,
			[
				tx({
					bookingDate: '2026-02-05',
					amount: '-99.00',
					label: { status: 'NEEDS_REVIEW', category: null } as never
				}),
				tx({ bookingDate: '2025-12-31', amount: '-5.00' }),
				tx({ bookingDate: '2026-03-01', amount: '-5.00' })
			],
			'2026-02-01',
			'2026-02-28'
		);
		expect(series.map((p) => p.total)).toEqual([0, 0]);
	});
});

describe('goal form', () => {
	const valid = () => ({
		...emptyGoalForm(),
		name: 'Hobbies',
		amount: '200',
		categorySlugs: ['entertainment.games']
	});

	it('an empty form fails on name, amount and scope', () => {
		expect(Object.keys(validateGoalForm(emptyGoalForm())).sort()).toEqual([
			'amount',
			'name',
			'scope'
		]);
	});

	it('a valid recurring form has no errors', () => {
		expect(validateGoalForm(valid())).toEqual({});
	});

	it('rejects zero, negative and malformed amounts', () => {
		for (const amount of ['0', '0.00', '-5', 'abc', '1.23456', '']) {
			expect(validateGoalForm({ ...valid(), amount }).amount).toBeDefined();
		}
	});

	it('tags alone satisfy the scope; tags that normalize to nothing do not', () => {
		const noCats = { ...valid(), categorySlugs: [] };
		expect(validateGoalForm({ ...noCats, tags: ['hobby'] }).scope).toBeUndefined();
		expect(validateGoalForm({ ...noCats, tags: ['!'] }).scope).toBeDefined();
	});

	it('a fixed goal needs a start, and an end not before it', () => {
		const f = { ...valid(), periodKind: 'FIXED' as const };
		expect(validateGoalForm(f).startDate).toBeDefined();
		expect(
			validateGoalForm({ ...f, startDate: '2026-02-01', endDate: '2026-01-01' }).endDate
		).toBeDefined();
		expect(validateGoalForm({ ...f, startDate: '2026-02-01' })).toEqual({});
	});

	it('recurring input carries cadence and no dates', () => {
		const input = formToInput({ ...valid(), tags: ['Hobby', 'hobby'] });
		expect(input).toMatchObject({
			name: 'Hobbies',
			amount: '200.00',
			tags: ['hobby'],
			periodKind: 'RECURRING',
			cadence: 'MONTHLY'
		});
		expect('startDate' in input).toBe(false);
	});

	it('fixed input carries dates and no cadence; blank end is null', () => {
		const input = formToInput({
			...valid(),
			periodKind: 'FIXED',
			startDate: '2026-01-01',
			endDate: ''
		});
		expect(input.startDate).toBe('2026-01-01');
		expect(input.endDate).toBeNull();
		expect('cadence' in input).toBe(false);
	});

	it('round-trips a goal through the form', () => {
		const form = goalToForm(goals[0]);
		expect(form.categorySlugs).toEqual(['entertainment.games', 'entertainment.movies_concerts']);
		expect(form.amount).toBe('200.00');
		expect(form.combine).toBe('ANY');
		expect(formToInput(form)).toMatchObject({
			type: 'SPENDING_LIMIT',
			amount: '200.00',
			periodKind: 'RECURRING',
			cadence: 'MONTHLY'
		});
	});
});

describe('tone presentation', () => {
	it('maps tones to token-backed classes', () => {
		expect(toneTextClass('good')).toContain('--color-income');
		expect(toneTextClass('bad')).toContain('--color-spending');
		expect(toneBarClass('neutral')).toContain('--color-muted');
	});

	it('words never depend on colour', () => {
		expect(toneText('SPENDING_LIMIT', 'bad', true)).toBe('Over limit');
		expect(toneText('SPENDING_LIMIT', 'good', false)).toBe('Under limit');
		expect(toneText('SAVING_TARGET', 'neutral', true)).toBe('Not there yet');
		expect(toneText('SAVING_TARGET', 'bad', false)).toBe('Target missed');
		expect(toneText('SAVING_TARGET', 'good', false)).toBe('Target reached');
	});
});
