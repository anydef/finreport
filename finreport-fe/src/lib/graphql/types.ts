/**
 * Hand-maintained TypeScript types mirroring the frozen GraphQL SDL
 * (`src/lib/graphql/schema.graphql`, WP0-owned — see docs/specs/iteration-1.md
 * §5). Kept intentionally small/hand-written rather than codegen'd: iteration
 * 1 has a handful of operations and codegen tooling is out of scope here.
 *
 * `Decimal`/`Date`/`UUID` all travel over GraphQL as JSON strings.
 */

export type UUID = string;
/** "YYYY-MM-DD". */
export type DateString = string;
/** Exact decimal as a string, e.g. "-12.3400". */
export type Decimal = string;

export type Direction = 'INCOME' | 'SPENDING';
export type Granularity = 'DAY' | 'WEEK' | 'MONTH';

export type CashflowNodeKind =
	| 'INCOME_SOURCE'
	| 'ACCOUNT'
	| 'SPENDING'
	| 'NET'
	| 'DEFICIT'
	| 'OTHER'
	| 'CATEGORY'
	| 'TAG';

export type CashflowDimension = 'INCOME_SOURCE' | 'ACCOUNT' | 'OUTCOME' | 'CATEGORY' | 'TAG';

export interface Me {
	id: UUID;
	username: string;
	displayName: string | null;
}

export interface Balance {
	date: DateString;
	amount: Decimal;
	currency: string;
}

export interface Account {
	id: UUID;
	source: string;
	externalId: string;
	displayId: string | null;
	accountType: string | null;
	iban: string | null;
	bic: string | null;
	institute: string | null;
	label: string | null;
	currency: string;
	latestBalance: Balance | null;
}

export interface Transaction {
	id: UUID;
	accountId: UUID;
	source: string;
	externalId: string;
	bookingDate: DateString;
	valutaDate: DateString | null;
	bookingStatus: string;
	amount: Decimal;
	currency: string;
	counterpartyName: string | null;
	counterpartyIban: string | null;
	description: string | null;
	transactionType: string | null;
}

export interface TransactionFilter {
	startDate?: DateString | null;
	endDate?: DateString | null;
	accountIds?: UUID[] | null;
	search?: string | null;
	direction?: Direction | null;
	counterpartyNames?: string[] | null;
	hasCounterparty?: boolean | null;
}

export interface PageInput {
	limit: number;
	offset: number;
}

export interface TransactionPage {
	items: Transaction[];
	totalCount: number;
	limit: number;
	offset: number;
}

export interface CashflowBucket {
	start: DateString;
	end: DateString;
	income: Decimal;
	spending: Decimal;
	net: Decimal;
	transactionCount: number;
}

export interface CashflowTotals {
	income: Decimal;
	spending: Decimal;
	net: Decimal;
	transactionCount: number;
}

export interface CashflowSummary {
	buckets: CashflowBucket[];
	total: CashflowTotals;
	currency: string;
}

export interface CashflowGraphInput {
	dimensions?: CashflowDimension[];
	maxNodesPerDimension?: number;
}

export interface CashflowNode {
	id: string;
	label: string;
	kind: CashflowNodeKind;
	depth: number;
	value: Decimal;
	refType: string | null;
	refId: string | null;
}

export interface CashflowLink {
	sourceId: string;
	targetId: string;
	value: Decimal;
}

export interface CashflowGraph {
	nodes: CashflowNode[];
	links: CashflowLink[];
	currency: string;
	dimensions: CashflowDimension[];
	truncated: boolean;
}
