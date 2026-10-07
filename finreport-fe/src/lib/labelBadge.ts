/**
 * Pure mapping from a `TransactionLabel` to the badge the transaction table
 * renders (§6 WP5): "text + shape, never colour alone" (iteration-1 a11y
 * rule, reiterated for iteration 2). Covered by `labelBadge.test.ts`.
 *
 * `TRANSACTIONS_QUERY` (frozen, WP0) does not fetch `label.rule`, so a
 * learned, auto-approved rule can't be told apart from a plain rule via
 * `rule.origin`/`rule.autoApproved`. Instead this uses the threshold from
 * §2.8: a `RULE`-sourced label with `confidence >= 0.9` is a learned rule
 * that cleared auto-approval, which is the same number the backend used to
 * decide auto-approval in the first place.
 */

import type { LabelSource, TransactionLabel } from './graphql/types';

/** Confidence at/above which a learned rule is auto-approved (§2.8). */
export const AUTO_APPROVE_THRESHOLD = 0.9;

export type BadgeVariant = 'neutral' | 'info' | 'success' | 'warning';

export interface LabelBadge {
	text: string;
	variant: BadgeVariant;
}

const SOURCE_BADGES: Record<LabelSource, LabelBadge> = {
	USER: { text: 'you', variant: 'neutral' },
	RULE: { text: 'rule', variant: 'info' },
	LLM_CACHE: { text: 'cached', variant: 'info' },
	LLM: { text: 'AI', variant: 'warning' }
};

/** Badge for a resolved/held label's *source* — `null` when there's no label at all yet. */
export function labelSourceBadge(label: TransactionLabel | null): LabelBadge | null {
	if (!label) return null;
	if (label.source === 'RULE' && (label.confidence ?? 0) >= AUTO_APPROVE_THRESHOLD) {
		return { text: 'auto-rule', variant: 'success' };
	}
	return SOURCE_BADGES[label.source];
}

/** The separate "needs review" pill (§6) — shown alongside, never instead of, the source badge. */
export function needsReviewBadge(label: TransactionLabel | null): LabelBadge | null {
	if (label?.status === 'NEEDS_REVIEW') return { text: 'needs review', variant: 'warning' };
	return null;
}
