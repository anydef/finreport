import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import {
	colorForKind,
	decimalToNumber,
	drilldownFilterForLink,
	drilldownFilterForNode,
	normalizeNodeKind,
	shapeCashflowBars,
	shapeCashflowGraph,
	type ShapedSankeyNode
} from './chartShaping';
import type { CashflowGraph, CashflowSummary } from './graphql/types';

function loadMock<T>(name: string): { variables: unknown; data: T } {
	const file = path.resolve(__dirname, 'graphql/mocks', name);
	return JSON.parse(readFileSync(file, 'utf-8'));
}

describe('decimalToNumber', () => {
	it('parses a Decimal string', () => {
		expect(decimalToNumber('1550.00')).toBe(1550);
	});

	it('parses a negative Decimal string', () => {
		expect(decimalToNumber('-12.34')).toBe(-12.34);
	});

	it('falls back to 0 for unparseable input', () => {
		expect(decimalToNumber('not-a-number')).toBe(0);
	});
});

describe('shapeCashflowBars', () => {
	const mock = loadMock<{ cashflowSummary: CashflowSummary }>('cashflow-summary.json');

	it('shapes buckets into a diverging income/spending dataset', () => {
		const bars = shapeCashflowBars(mock.data.cashflowSummary, 'MONTH');
		expect(bars).toEqual([
			{
				label: 'Jan 2024',
				start: '2024-01-01',
				end: '2024-01-31',
				income: 2500,
				spending: -950,
				net: 1550,
				transactionCount: 2
			}
		]);
	});

	it('returns an empty array for an empty bucket list', () => {
		expect(shapeCashflowBars({ buckets: [] }, 'DAY')).toEqual([]);
	});
});

describe('normalizeNodeKind', () => {
	it('passes through known kinds', () => {
		expect(normalizeNodeKind('DEFICIT')).toBe('DEFICIT');
		expect(normalizeNodeKind('NET')).toBe('NET');
		expect(normalizeNodeKind('OTHER')).toBe('OTHER');
	});

	it('falls back to OTHER for an unknown/future kind', () => {
		expect(normalizeNodeKind('SOME_FUTURE_KIND')).toBe('OTHER');
	});
});

describe('shapeCashflowGraph', () => {
	const netDeficit = loadMock<{ cashflowGraph: CashflowGraph }>('cashflow-graph-net-deficit.json')
		.data.cashflowGraph;
	const truncated = loadMock<{ cashflowGraph: CashflowGraph }>('cashflow-graph-truncated.json').data
		.cashflowGraph;

	it('shapes nodes/links into plain source/target records', () => {
		const shaped = shapeCashflowGraph(netDeficit);
		expect(shaped.nodes).toHaveLength(netDeficit.nodes.length);
		expect(shaped.links[0]).toEqual({
			source: 'income:acme-gmbh',
			target: 'account:2b0fe9c2-2f4b-5753-95ad-133501e3bd5d',
			value: 15000
		});
	});

	it('labels account nodes with the account display name, leaving drill-down refs alone', () => {
		const id = '2b0fe9c2-2f4b-5753-95ad-133501e3bd5d';
		const shaped = shapeCashflowGraph(netDeficit, [{ id, displayName: 'Pavlo - everyday' }]);
		const node = shaped.nodes.find((n) => n.refType === 'account' && n.refId === id);
		expect(node?.label).toBe('Pavlo - everyday');
		expect(node && drilldownFilterForNode(node)).toEqual({ accountIds: [id] });
		expect(shapeCashflowGraph(netDeficit).nodes.find((n) => n.refId === id)?.label).toBe('Main');
	});

	it('includes both a NET and a DEFICIT account node in the same graph', () => {
		const shaped = shapeCashflowGraph(netDeficit);
		const kinds = shaped.nodes.filter((n) => n.refType === 'account').map((n) => n.kind);
		expect(kinds).toContain('NET');
		expect(kinds).toContain('DEFICIT');
	});

	it('preserves the truncated flag and an Other node', () => {
		const shaped = shapeCashflowGraph(truncated);
		expect(shaped.truncated).toBe(true);
		expect(shaped.nodes.some((n) => n.kind === 'OTHER' && n.label === 'Other')).toBe(true);
	});

	it('preserves an Unknown-counterparty node', () => {
		const shaped = shapeCashflowGraph(truncated);
		expect(shaped.nodes.some((n) => n.label === 'Unknown')).toBe(true);
	});
});

describe('colorForKind', () => {
	it('maps income-like kinds to the income color token', () => {
		expect(colorForKind('INCOME_SOURCE')).toBe('var(--color-income)');
		expect(colorForKind('NET')).toBe('var(--color-income)');
	});

	it('maps spending-like kinds to the spending color token', () => {
		expect(colorForKind('SPENDING')).toBe('var(--color-spending)');
		expect(colorForKind('DEFICIT')).toBe('var(--color-spending)');
	});

	it('falls back to the muted token for OTHER/CATEGORY/TAG', () => {
		expect(colorForKind('OTHER')).toBe('var(--color-muted)');
		expect(colorForKind('CATEGORY')).toBe('var(--color-muted)');
	});
});

describe('drilldownFilterForNode', () => {
	const accountNode: ShapedSankeyNode = {
		id: 'account:a1',
		label: 'Main',
		kind: 'NET',
		depth: 1,
		value: 100,
		refType: 'account',
		refId: 'a1'
	};
	const unknownNode: ShapedSankeyNode = {
		id: 'outcome:unknown',
		label: 'Unknown',
		kind: 'SPENDING',
		depth: 2,
		value: 10,
		refType: null,
		refId: null
	};
	const otherNode: ShapedSankeyNode = {
		id: 'outcome:other',
		label: 'Other',
		kind: 'OTHER',
		depth: 2,
		value: 10,
		refType: null,
		refId: null
	};
	const counterpartyNode: ShapedSankeyNode = {
		id: 'outcome:acme',
		label: 'ACME GmbH',
		kind: 'SPENDING',
		depth: 2,
		value: 10,
		refType: null,
		refId: null
	};

	it('an account node (even DEFICIT/NET) narrows by accountIds, not kind', () => {
		expect(drilldownFilterForNode(accountNode)).toEqual({ accountIds: ['a1'] });
	});

	it('an Unknown node narrows via hasCounterparty: false', () => {
		expect(drilldownFilterForNode(unknownNode)).toEqual({ hasCounterparty: false });
	});

	it('an Other node has no single identity to drill into', () => {
		expect(drilldownFilterForNode(otherNode)).toBeNull();
	});

	it('a named counterparty node narrows by exact counterpartyNames', () => {
		expect(drilldownFilterForNode(counterpartyNode)).toEqual({
			counterpartyNames: ['ACME GmbH']
		});
	});
});

describe('drilldownFilterForLink', () => {
	const account: ShapedSankeyNode = {
		id: 'account:a1',
		label: 'Main',
		kind: 'DEFICIT',
		depth: 1,
		value: 100,
		refType: 'account',
		refId: 'a1'
	};
	const counterparty: ShapedSankeyNode = {
		id: 'outcome:acme',
		label: 'ACME GmbH',
		kind: 'SPENDING',
		depth: 2,
		value: 10,
		refType: null,
		refId: null
	};
	const other: ShapedSankeyNode = {
		id: 'outcome:other',
		label: 'Other',
		kind: 'OTHER',
		depth: 2,
		value: 10,
		refType: null,
		refId: null
	};

	it('merges both ends of a link into one filter', () => {
		expect(drilldownFilterForLink(account, counterparty)).toEqual({
			accountIds: ['a1'],
			counterpartyNames: ['ACME GmbH']
		});
	});

	it('drops a side that resolves to null (e.g. an Other endpoint)', () => {
		expect(drilldownFilterForLink(account, other)).toEqual({ accountIds: ['a1'] });
	});

	it('returns null when both ends resolve to null', () => {
		expect(drilldownFilterForLink(other, other)).toBeNull();
	});
});
