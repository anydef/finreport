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
	isAdmin: boolean;
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
	/** Sorted; `[]` when untagged (iteration 3 §4). */
	tags: string[];
	/** `null` = not an internal transfer (iteration 3 §4). */
	transfer: TransferInfo | null;
	/** Always present; `isRecurring` may be `false` (iteration 3 §4). */
	recurring: RecurringInfo;
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
	/** AND-ed: the transaction carries all of them (iteration 3 §4). */
	tags?: string[] | null;
	/** Matches the *effective* flag (iteration 3 §4). */
	recurring?: boolean | null;
	/** `true` = only transfers, `false` = only non-transfers (iteration 3 §4). */
	transfer?: boolean | null;
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

// ---------------------------------------------------------------------------
// Tags, internal transfers, recurring costs (iteration 3, §4)
// ---------------------------------------------------------------------------

export type FlagSource = 'AUTO' | 'USER';
export type RecurringCadence = 'MONTHLY' | 'QUARTERLY' | 'YEARLY';
export type TransferMatchKind = 'IBAN' | 'AMOUNT_DATE';

export interface TransferInfo {
	/** `null` while the other leg is not projected yet. */
	counterpartTransactionId: UUID | null;
	counterpartAccountId: UUID | null;
	match: TransferMatchKind;
}

/** `source = 'USER'` when overridden; `seriesId` is `null` when overridden
 * in or out of a series. */
export interface RecurringInfo {
	isRecurring: boolean;
	source: FlagSource;
	seriesId: UUID | null;
	cadence: RecurringCadence | null;
	medianAmount: Decimal | null;
}

export interface RecurringSeries {
	id: UUID;
	counterpartyKey: string;
	counterpartyName: string | null;
	direction: Direction;
	cadence: RecurringCadence;
	/** Signed. */
	medianAmount: Decimal;
	/** Signed, 4 dp. */
	monthlyEquivalent: Decimal;
	occurrenceCount: number;
	firstDate: DateString;
	lastDate: DateString;
	nextExpectedDate: DateString;
	/** `lastDate` older than cadence + grace. */
	stale: boolean;
}

export interface RecurringOverview {
	series: RecurringSeries[];
	/** Expense series only, positive magnitude. */
	totalMonthlyEquivalent: Decimal;
	currency: string;
}

export interface TagCount {
	tag: string;
	transactionCount: number;
}

// ---------------------------------------------------------------------------
// Goals (iteration 4, §4)
// ---------------------------------------------------------------------------

export type GoalType = 'SPENDING_LIMIT' | 'SAVING_TARGET';
export type GoalPeriodKind = 'RECURRING' | 'FIXED';
export type GoalCadence = 'MONTHLY' | 'QUARTERLY' | 'YEARLY';
export type ScopeCombine = 'ALL' | 'ANY';

export interface GoalScope {
	/** Resolved from slugs; archived ones are still listed. */
	categories: Category[];
	tags: string[];
	combine: ScopeCombine;
	tagCombine: ScopeCombine;
}

export interface Goal {
	id: UUID;
	name: string;
	type: GoalType;
	amount: Decimal;
	currency: string;
	scope: GoalScope;
	periodKind: GoalPeriodKind;
	/** `null` for `FIXED`. */
	cadence: GoalCadence | null;
	/** `null` for `RECURRING`. */
	startDate: DateString | null;
	/** `null` for `RECURRING` or an open-ended `FIXED`. */
	endDate: DateString | null;
	archived: boolean;
}

export interface GoalBucket {
	start: DateString;
	end: DateString;
	label: string;
	/** Positive magnitude. */
	total: Decimal;
	/** Held-for-review, excluded from `total`. */
	pending: Decimal;
	/** `amount - total`; negative when over. */
	remaining: Decimal;
	/** `<= amount` for a limit, `>= amount` for a target. */
	met: boolean;
	inProgress: boolean;
}

export interface GoalProgress {
	goal: Goal;
	buckets: GoalBucket[];
	/** Across every bucket. */
	total: Decimal;
	pending: Decimal;
	/** Over completed buckets only. */
	averagePerPeriod: Decimal;
	currency: string;
}

/** Mirrors the `GoalInput` SDL input; `RECURRING` needs `cadence`, `FIXED` needs `startDate`. */
export interface GoalInput {
	name: string;
	type: GoalType;
	amount: Decimal;
	currency?: string;
	categorySlugs?: string[];
	tags?: string[];
	combine?: ScopeCombine;
	tagCombine?: ScopeCombine;
	periodKind: GoalPeriodKind;
	cadence?: GoalCadence | null;
	startDate?: DateString | null;
	endDate?: DateString | null;
}
