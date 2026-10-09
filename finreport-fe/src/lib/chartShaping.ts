/**
 * Pure, non-Svelte shaping of GraphQL responses into chart-ready data (§6, §9
 * WP5). Covered by `chartShaping.test.ts`, including the DEFICIT/Other/Unknown
 * edge cases the WP0 fixtures deliberately exercise.
 */

import type { Granularity } from './period';
import { bucketLabel } from './period';
import { relabelAccountNodes } from './displayNames';
import type {
	CashflowBucket,
	CashflowGraph,
	CashflowLink,
	CashflowNode,
	CashflowNodeKind,
	CashflowSummary,
	Account,
	TransactionFilter
} from './graphql/types';

/** Parse a `Decimal` (exact-decimal string) into a JS number for charting. */
export function decimalToNumber(value: string): number {
	const n = Number(value);
	return Number.isNaN(n) ? 0 : n;
}

export interface CashflowBarDatum {
	label: string;
	start: string;
	end: string;
	income: number;
	/** Negative, so it plots below the axis in a diverging bar chart. */
	spending: number;
	net: number;
	transactionCount: number;
}

/** Shape `cashflowSummary.buckets` into a diverging income/spending bar dataset. */
export function shapeCashflowBars(
	summary: Pick<CashflowSummary, 'buckets'>,
	granularity: Granularity
): CashflowBarDatum[] {
	return summary.buckets.map((bucket: CashflowBucket) => ({
		label: bucketLabel(bucket, granularity),
		start: bucket.start,
		end: bucket.end,
		income: decimalToNumber(bucket.income),
		spending: -decimalToNumber(bucket.spending),
		net: decimalToNumber(bucket.net),
		transactionCount: bucket.transactionCount
	}));
}

/** Known node kinds from the current SDL (§5). Anything else is a future/unknown kind. */
const KNOWN_KINDS: ReadonlySet<CashflowNodeKind> = new Set([
	'INCOME_SOURCE',
	'ACCOUNT',
	'SPENDING',
	'NET',
	'DEFICIT',
	'OTHER',
	'CATEGORY',
	'TAG'
]);

/**
 * Normalize a node's `kind` to a known value, falling back to `OTHER` for
 * anything the frontend doesn't recognize yet (§5: "clients treat unknown
 * `kind` values as `OTHER`").
 */
export function normalizeNodeKind(kind: string): CashflowNodeKind {
	return KNOWN_KINDS.has(kind as CashflowNodeKind) ? (kind as CashflowNodeKind) : 'OTHER';
}

export interface ShapedSankeyNode {
	id: string;
	label: string;
	kind: CashflowNodeKind;
	depth: number;
	value: number;
	refType: string | null;
	refId: string | null;
}

export interface ShapedSankeyLink {
	source: string;
	target: string;
	value: number;
}

export interface ShapedSankeyGraph {
	nodes: ShapedSankeyNode[];
	links: ShapedSankeyLink[];
	currency: string;
	truncated: boolean;
}

/**
 * Shape `cashflowGraph` into the plain node/link records LayerChart's Sankey
 * layout consumes (`nodeId`/`source`/`target` as string ids).
 */
export function shapeCashflowGraph(
	graph: CashflowGraph,
	accounts: Pick<Account, 'id' | 'displayName'>[] = []
): ShapedSankeyGraph {
	return {
		nodes: relabelAccountNodes(
			graph.nodes.map((node: CashflowNode) => ({
				id: node.id,
				label: node.label,
				kind: normalizeNodeKind(node.kind),
				depth: node.depth,
				value: decimalToNumber(node.value),
				refType: node.refType,
				refId: node.refId
			})),
			accounts
		),
		links: graph.links.map((link: CashflowLink) => ({
			source: link.sourceId,
			target: link.targetId,
			value: decimalToNumber(link.value)
		})),
		currency: graph.currency,
		truncated: graph.truncated
	};
}

/** Tailwind-token colour class for a node/bar of a given cashflow kind. */
export function colorForKind(kind: CashflowNodeKind): string {
	switch (kind) {
		case 'INCOME_SOURCE':
		case 'NET':
			return 'var(--color-income)';
		case 'SPENDING':
		case 'DEFICIT':
			return 'var(--color-spending)';
		case 'ACCOUNT':
			return 'var(--color-account)';
		default:
			return 'var(--color-muted)';
	}
}

/**
 * Build the `TransactionFilter` narrowing produced by clicking a Sankey node:
 * an `ACCOUNT` node narrows to that account, an `Unknown` node (no counterparty)
 * narrows via `hasCounterparty: false`, a named income/outcome node narrows by
 * exact counterparty name. `Other`/synthetic `NET`/`DEFICIT` nodes have no
 * single identity to drill into, so they return `null` (no further narrowing).
 */
export function drilldownFilterForNode(node: ShapedSankeyNode): Partial<TransactionFilter> | null {
	if (node.refType === 'account' && node.refId) {
		return { accountIds: [node.refId] };
	}
	// `CATEGORY`-dimension nodes drill down through `categorySlugs` (§5); an
	// `Uncategorized` synthetic node (no `refId`) narrows via `uncategorized`
	// instead so it doesn't silently fall through to the generic branches
	// below.
	if (node.kind === 'CATEGORY') {
		if (node.refType === 'category' && node.refId) return { categorySlugs: [node.refId] };
		return { uncategorized: true };
	}
	if (node.kind === 'OTHER' || node.kind === 'NET' || node.kind === 'DEFICIT') {
		return null;
	}
	if (node.label === 'Unknown') {
		return { hasCounterparty: false };
	}
	if (node.kind === 'INCOME_SOURCE' || node.kind === 'SPENDING') {
		return { counterpartyNames: [node.label] };
	}
	return null;
}

/**
 * Build the filter for clicking a link: the intersection (both ends') of
 * whatever each endpoint node resolves to, merged into one filter. Either
 * side contributing `null` (e.g. an `Other` endpoint) drops that side's
 * narrowing rather than failing the whole merge.
 */
export function drilldownFilterForLink(
	sourceNode: ShapedSankeyNode | undefined,
	targetNode: ShapedSankeyNode | undefined
): Partial<TransactionFilter> | null {
	const sourceFilter = sourceNode ? drilldownFilterForNode(sourceNode) : null;
	const targetFilter = targetNode ? drilldownFilterForNode(targetNode) : null;
	if (!sourceFilter && !targetFilter) return null;
	return { ...sourceFilter, ...targetFilter };
}
