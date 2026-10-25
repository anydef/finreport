/**
 * Pure shaping of a `CounterpartyBreakdown` response into the rows the
 * dashboard's Counterparty tab renders. No Svelte, no I/O - covered by
 * `counterpartyView.test.ts`.
 *
 * Unlike the category breakdown this one has no `share`: rows mix signs (an
 * employer is income, a shop is spending), so a percentage of the total would
 * mean nothing. Amounts are signed nets and stay signed.
 *
 * IBANs: a row is keyed by (counterpartyKey, counterpartyIban) because the key
 * is name-derived and can collapse distinct accounts (e.g. the user's own loan
 * account) into one merchant. The IBAN is shown, masked, only when it tells a
 * row from a sibling (`ibanDisambiguated`). Masking is display only.
 *
 * PRIVACY TRADEOFF - do not "fix" by masking the param: the drill-down puts the
 * RAW IBAN in the `/transactions` query string (`counterpartyIbans`), so it
 * lands in browser history and in any request log of the deployed adapter-node
 * frontend. Nothing in our own code logs it. That is the price of exact
 * filtering; a masked value cannot identify a row.
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
	/** Stable `#each` key; encodes key and IBAN, since one key can span several rows. */
	key: string;
	label: string;
	kind: CounterpartyRowKind;
	/** The merchant key to filter by; `null` for the no-key and `other` rows. */
	counterpartyKey: string | null;
	/** Raw IBAN for the drill-down only; never render it. */
	counterpartyIban: string | null;
	/** Masked IBAN, present only when it disambiguates this row from a sibling. */
	ibanMasked: string | null;
	amount: number;
	/** Signed display text, e.g. `+120.00 EUR` / `−45.50 EUR`; zero has no sign. */
	amountText: string;
	tone: AmountTone;
	transactionCount: number;
	/** Whether this row's key is split across IBANs (decides the drill-down filter). */
	ibanDisambiguated: boolean;
	/** Whether the row can drill into `/transactions`. */
	drillable: boolean;
}

export function amountTone(amount: number): AmountTone {
	if (amount > 0) return 'income';
	if (amount < 0) return 'spending';
	return 'neutral';
}

/**
 * Mask an IBAN for display: `•••• ` plus the last 4 characters. Values of 4 or
 * fewer characters (after stripping whitespace) are returned as-is. Never throws.
 */
export function maskIban(iban: string | null | undefined): string | null {
	if (typeof iban !== 'string') return null;
	const compact = iban.replace(/\s+/g, '');
	if (compact === '') return null;
	return compact.length <= 4 ? compact : `•••• ${compact.slice(-4)}`;
}

/**
 * A row is drillable when it has a key or an IBAN (a no-key row carrying an
 * IBAN is exactly identifiable). The `other` tail sums many counterparties and
 * no filter expresses it.
 */
export function isDrillable(row: {
	kind: CounterpartyRowKind;
	counterpartyKey: string | null;
	counterpartyIban?: string | null;
}) {
	if (row.kind === 'other') return false;
	return Boolean(row.counterpartyKey) || Boolean(row.counterpartyIban);
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
		counterpartyKey: kind === 'other' ? null : row.counterpartyKey,
		counterpartyIban: kind === 'other' ? null : (row.counterpartyIban ?? null)
	};
	return {
		key:
			kind === 'other'
				? kind
				: `${kind}:${row.counterpartyKey ?? ''}:${row.counterpartyIban ?? ''}:${index}`,
		label: row.displayName,
		...view,
		ibanMasked: kind !== 'other' && row.ibanDisambiguated ? maskIban(row.counterpartyIban) : null,
		ibanDisambiguated: row.ibanDisambiguated,
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

/** The `/transactions` search param carrying exact IBANs (comma-separated). */
export const COUNTERPARTY_IBANS_PARAM = 'counterpartyIbans';
/** The `/transactions` search param for `hasCounterpartyIban` (`true` / `false`). */
export const HAS_COUNTERPARTY_IBAN_PARAM = 'hasCounterpartyIban';

export function parseCounterpartyKeys(params: URLSearchParams): string[] {
	return (params.get(COUNTERPARTY_KEYS_PARAM) ?? '').split(',').filter(Boolean);
}

export function parseCounterpartyIbans(params: URLSearchParams): string[] {
	return (params.get(COUNTERPARTY_IBANS_PARAM) ?? '').split(',').filter(Boolean);
}

/** Only `'true'`/`'false'` constrain; anything else is `undefined`, never a silent `false`. */
export function parseHasCounterpartyIban(params: URLSearchParams): boolean | undefined {
	const v = params.get(HAS_COUNTERPARTY_IBAN_PARAM);
	if (v === 'true') return true;
	if (v === 'false') return false;
	return undefined;
}

export interface DrillScope {
	preset: string;
	start: string;
	end: string;
	panel: PanelFilters;
}

/**
 * Link to `/transactions` for one row, keeping the dashboard's period and
 * filter panel. The filter selects exactly the transactions the row counted:
 * key alone, key + IBAN, key + no IBAN (`hasCounterpartyIban=false`), or IBAN
 * alone for a no-key row. Returns `null` for a row that is not drillable.
 */
export function counterpartyHref(
	row: Pick<CounterpartyRowView, 'kind' | 'counterpartyKey'> &
		Partial<Pick<CounterpartyRowView, 'counterpartyIban' | 'ibanDisambiguated'>>,
	scope: DrillScope
): string | null {
	if (!isDrillable(row)) return null;
	const params = new URLSearchParams();
	if (scope.preset !== 'this-month') params.set('preset', scope.preset);
	if (scope.preset === 'custom') {
		params.set('start', scope.start);
		params.set('end', scope.end);
	}
	const withPanel = writePanelFilters(scope.panel, params);
	const key = row.counterpartyKey || null;
	const iban = row.counterpartyIban || null;
	if (key) {
		withPanel.set(COUNTERPARTY_KEYS_PARAM, key);
		if (row.ibanDisambiguated) {
			if (iban) withPanel.set(COUNTERPARTY_IBANS_PARAM, iban);
			else withPanel.set(HAS_COUNTERPARTY_IBAN_PARAM, 'false');
		}
	} else if (iban) {
		withPanel.set(COUNTERPARTY_IBANS_PARAM, iban);
	}
	return `/transactions?${withPanel.toString()}`;
}

/** The "Narrowed to merchant" text for the `/transactions` page; IBAN masked. */
export function narrowedToLabel(
	keys: string[],
	ibans: string[],
	hasIban: boolean | undefined
): string {
	const parts = keys.length ? [keys.join(', ')] : [];
	const masked = ibans.map((i) => maskIban(i)).filter((m): m is string => m !== null);
	if (masked.length) parts.push(`(${masked.join(', ')})`);
	else if (hasIban === false) parts.push('(no IBAN)');
	else if (hasIban === true) parts.push('(with IBAN)');
	return parts.join(' ');
}
