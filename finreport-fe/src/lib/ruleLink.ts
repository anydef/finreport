/**
 * Pure decision behind the "set by rule" link in the transaction editor:
 * whether to show one, where it points, and what it says when the rule has
 * since been deleted. Covered by `ruleLink.test.ts`.
 *
 * The label only carries `rule` when the rule path produced it; a
 * `source: RULE` label whose `rule` is `null` means the row it pointed at
 * no longer exists (rules can be deleted while labels keep their `rule_id`).
 */

import type { TransactionLabel } from './graphql/types';

export interface RuleLink {
	/** `null` when the rule is gone: the editor then renders plain text, not a link. */
	href: string | null;
	text: string;
}

/** `/admin/rules` scoped to one rule: the page highlights and scrolls to that row. */
export function ruleHref(ruleId: string): string {
	return `/admin/rules?rule=${encodeURIComponent(ruleId)}`;
}

/** `null` unless the label actually came from a rule. */
export function ruleLinkFor(label: TransactionLabel | null | undefined): RuleLink | null {
	if (!label || label.source !== 'RULE') return null;
	if (!label.rule) return { href: null, text: 'Set by a rule that has since been deleted' };
	return { href: ruleHref(label.rule.id), text: `Set by rule: ${label.rule.name}` };
}

/** The `?rule=` search param of `/admin/rules`, or `null` when absent/blank. */
export function highlightedRuleId(params: URLSearchParams): string | null {
	const id = params.get('rule')?.trim();
	return id ? id : null;
}
