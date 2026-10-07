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

export type CategoryKind = 'INCOME' | 'EXPENSE' | 'TRANSFER' | 'SAVING';
export type LabelSource = 'USER' | 'RULE' | 'LLM_CACHE' | 'LLM';
export type LabelStatus = 'RESOLVED' | 'NEEDS_REVIEW';
export type ReviewReason = 'AMBIGUOUS' | 'NEW_CATEGORY';
export type RuleState = 'ACTIVE' | 'IN_REVIEW' | 'REVOKED' | 'REJECTED';
export type RuleOrigin = 'USER' | 'LEARNED';
/** Arbitrary JSON, e.g. `rule.conditions` — travels as a plain JS value. */
export type Json = unknown;

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
	/** `null` = not labelled yet (distinct from `needsReview`). */
	label: TransactionLabel | null;
	/** Empty when not split. */
	splits: TransactionSplit[];
}

export interface TransactionFilter {
	startDate?: DateString | null;
	endDate?: DateString | null;
	accountIds?: UUID[] | null;
	search?: string | null;
	direction?: Direction | null;
	counterpartyNames?: string[] | null;
	hasCounterparty?: boolean | null;
	/** OR-ed; includes descendants. */
	categorySlugs?: string[] | null;
	/** `true` = no label at all. */
	uncategorized?: boolean | null;
	needsReview?: boolean | null;
	labelSources?: LabelSource[] | null;
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

// ---------------------------------------------------------------------------
// Categories, labels, rules (iteration 2, §5)
// ---------------------------------------------------------------------------

export interface Category {
	id: UUID;
	slug: string;
	name: string;
	kind: CategoryKind;
	parentId: UUID | null;
	depth: number;
	archived: boolean;
	/** `"seed"` | `"user"`. */
	origin: string;
}

export interface Rule {
	id: UUID;
	name: string;
	category: Category;
	conditions: Json;
	priority: number;
	state: RuleState;
	origin: RuleOrigin;
	autoApproved: boolean;
	confidence: number | null;
	evidenceCount: number;
	/** RFC 3339 instant. */
	createdAt: string;
}

export interface TransactionLabel {
	/** `null` while `status = NEEDS_REVIEW`. */
	category: Category | null;
	source: LabelSource;
	rule: Rule | null;
	confidence: number | null;
	status: LabelStatus;
	reviewReason: ReviewReason | null;
	proposedCategoryPath: string | null;
	reasoning: string | null;
}

export interface TransactionSplit {
	index: number;
	amount: Decimal;
	category: Category;
}

export interface CategoryBreakdownRow {
	/** The roll-up level that was requested. */
	category: Category;
	/** Positive magnitude. */
	amount: Decimal;
	transactionCount: number;
	/** Of the row's `kind` total, `0..1`. */
	share: number;
}

export interface CategoryBreakdown {
	rows: CategoryBreakdownRow[];
	/** Label missing entirely. */
	uncategorized: CategoryBreakdownRow | null;
	/** Held; counted separately, never as spend. */
	needsReview: CategoryBreakdownRow | null;
	currency: string;
}

export interface ReviewQueue {
	/** `status = NEEDS_REVIEW`. */
	transactions: Transaction[];
	/** `state = IN_REVIEW`. */
	pendingRules: Rule[];
	totalCount: number;
}

export interface SplitPartInput {
	amount: Decimal;
	categorySlug: string;
}

export interface CategoryInput {
	slug: string;
	name: string;
	kind: CategoryKind;
	parentSlug?: string | null;
}

export interface RuleInput {
	name: string;
	categorySlug: string;
	conditions: Json;
	priority?: number;
}
