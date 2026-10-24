/**
 * Pure shaping of a `CounterpartyBreakdown` response into the rows the
 * dashboard's Counterparty tab renders. No Svelte, no I/O - covered by
 * `counterpartyView.test.ts`.
 *
 * Unlike the category breakdown this one has no `share`: rows mix signs (an
 * employer is income, a shop is spending), so a percentage of the total would
 * mean nothing. Amounts are signed nets and stay signed.
 */

import { decimalToNumber } from './chartShaping';
import { formatAmount } from './format';
import type { CounterpartyBreakdown, CounterpartyBreakdownRow } from './graphql/types';
import { writePanelFilters, type PanelFilters } from './transactionFilters';

export type BreakdownDimension = 'category' | 'counterparty';

/** The search param that carries the selected tab; absent means `category`. */
export const BREAKDOWN_PARAM = 'breakdown';

export function parseBreakdownDimension(params: URLSearchParams): BreakdownDimension {
	return params.get(BREAKDOWN_PARAM) === 'counterparty' ? 'counterparty' : 'category';
}

/** Write the tab into a copy of `params`; the default is left out of the URL. */
export function writeBreakdownDimension(
	dimension: BreakdownDimension,
	params: URLSearchParams
): URLSearchParams {
	const next = new URLSearchParams(params);
	if (dimension === 'counterparty') next.set(BREAKDOWN_PARAM, dimension);
	else next.delete(BREAKDOWN_PARAM);
	return next;
}

export type CounterpartyRowKind = 'merchant' | 'no-key' | 'other';
export type AmountTone = 'income' | 'spending' | 'neutral';

export interface CounterpartyRowView {
	/** Stable `#each` key. */
	key: string;
	label: string;
	kind: CounterpartyRowKind;
	/** The merchant key to filter by; `null` for the no-key and `other` rows. */
	counterpartyKey: string | null;
	amount: number;
	/** Signed display text, e.g. `+120.00 EUR` / `−45.50 EUR`; zero has no sign. */
	amountText: string;
	tone: AmountTone;
	transactionCount: number;
	/** Whether the row can drill into `/transactions`. */
	drillable: boolean;
}

export function amountTone(amount: number): AmountTone {
	if (amount > 0) return 'income';
	if (amount < 0) return 'spending';
	return 'neutral';
}

/** Only a row with a real merchant key is drillable; no-key and tail rows are not. */
export function isDrillable(row: { kind: CounterpartyRowKind; counterpartyKey: string | null }) {
	return row.kind === 'merchant' && row.counterpartyKey !== null && row.counterpartyKey !== '';
}

function toView(
	row: CounterpartyBreakdownRow,
	kind: CounterpartyRowKind,
	index: number,
	currency: string
): CounterpartyRowView {
	const amount = decimalToNumber(row.amount);
	const view = {
		kind,
		counterpartyKey: kind === 'other' ? null : row.counterpartyKey
	};
	return {
		key: kind === 'merchant' ? `merchant:${row.counterpartyKey}:${index}` : kind,
		label: row.displayName,
		...view,
		amount,
		amountText: formatAmount(row.amount, currency),
		tone: amountTone(amount),
		transactionCount: row.transactionCount,
		drillable: isDrillable(view)
	};
}

/** Rows in the server's order, then the tail bucket last (when there is one). */
export function shapeCounterpartyBreakdown(
	breakdown: CounterpartyBreakdown
): CounterpartyRowView[] {
	const rows = breakdown.rows.map((row, i) =>
		toView(row, row.counterpartyKey === null ? 'no-key' : 'merchant', i, breakdown.currency)
	);
	if (breakdown.other) rows.push(toView(breakdown.other, 'other', rows.length, breakdown.currency));
	return rows;
}

export function transactionCountLabel(count: number): string {
	return `${count} ${count === 1 ? 'transaction' : 'transactions'}`;
}

// ------------------------------------------------------------------ drill-down

/** The `/transactions` search param carrying a merchant filter. */
export const COUNTERPARTY_KEYS_PARAM = 'counterpartyKeys';

export function parseCounterpartyKeys(params: URLSearchParams): string[] {
	return (params.get(COUNTERPARTY_KEYS_PARAM) ?? '').split(',').filter(Boolean);
}

export interface DrillScope {
	preset: string;
	start: string;
	end: string;
	panel: PanelFilters;
}

/**
 * Link to `/transactions` for one merchant, keeping the dashboard's period and
 * filter panel so the list holds exactly the rows the breakdown counted.
 * Returns `null` for a row that is not drillable.
 */
export function counterpartyHref(
	row: Pick<CounterpartyRowView, 'kind' | 'counterpartyKey'>,
	scope: DrillScope
): string | null {
	if (!isDrillable(row) || row.counterpartyKey === null) return null;
	const params = new URLSearchParams();
	if (scope.preset !== 'this-month') params.set('preset', scope.preset);
	if (scope.preset === 'custom') {
		params.set('start', scope.start);
		params.set('end', scope.end);
	}
	const withPanel = writePanelFilters(scope.panel, params);
	withPanel.set(COUNTERPARTY_KEYS_PARAM, row.counterpartyKey);
	return `/transactions?${withPanel.toString()}`;
}
