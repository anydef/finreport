/**
 * Mock-mode only: `mocks/transactions.json` (WP0, read-only) has no
 * rule-labelled row, so two are made at serve time to make the editor's
 * "set by rule" link reachable without a backend.
 *
 * - Lidl: labelled by the live "Rewe -> food.groceries" rule from
 *   `mocks/rules.json`, so the link lands on a real row of `/admin/rules`.
 * - Spotify: labelled by a rule that is not in `rules.json`, i.e. a deleted
 *   one, so the "rule has been deleted" wording is visible too.
 *
 * Every other transaction keeps its fixture label (no rule), which is the
 * "no link" case.
 */

const LIVE_RULE = {
	id: '6f5041b2-bd0e-4c9f-c081-4d5e6f718293',
	name: 'Rewe → food.groceries',
	state: 'ACTIVE'
};

const RULE_BY_TRANSACTION: Record<string, typeof LIVE_RULE | null> = {
	'6a9c1d2e-4b3f-5a6d-8c7e-9f0a1b2c3d51': LIVE_RULE, // Lidl
	'6a9c1d2e-4b3f-5a6d-8c7e-9f0a1b2c3d41': null // Spotify: its rule is gone
};

export function withMockRuleLabel<T extends { id: string; label: object | null }>(item: T): T {
	if (!(item.id in RULE_BY_TRANSACTION) || !item.label) return item;
	return {
		...item,
		label: { ...item.label, source: 'RULE', rule: RULE_BY_TRANSACTION[item.id] }
	};
}
