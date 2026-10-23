import { describe, expect, it } from 'vitest';
import { highlightedRuleId, ruleHref, ruleLinkFor } from './ruleLink';
import type { Rule, TransactionLabel } from './graphql/types';

function label(overrides: Partial<TransactionLabel>): TransactionLabel {
	return {
		category: null,
		source: 'RULE',
		rule: null,
		confidence: null,
		status: 'RESOLVED',
		reviewReason: null,
		proposedCategoryPath: null,
		reasoning: null,
		...overrides
	};
}

describe('ruleLinkFor', () => {
	it('links to the rule scoped on /admin/rules and names it', () => {
		const rule = { id: 'r-1', name: 'Rewe → food.groceries' } as Rule;
		expect(ruleLinkFor(label({ rule }))).toEqual({
			href: '/admin/rules?rule=r-1',
			text: 'Set by rule: Rewe → food.groceries'
		});
	});

	it('says the rule is gone, without a link, when source is RULE but the rule is missing', () => {
		const link = ruleLinkFor(label({ rule: null }));
		expect(link?.href).toBeNull();
		expect(link?.text).toMatch(/deleted/);
	});

	it('shows nothing for non-rule sources and unlabelled rows', () => {
		expect(ruleLinkFor(label({ source: 'USER' }))).toBeNull();
		expect(ruleLinkFor(label({ source: 'LLM' }))).toBeNull();
		expect(ruleLinkFor(null)).toBeNull();
		expect(ruleLinkFor(undefined)).toBeNull();
	});
});

describe('highlightedRuleId', () => {
	it('reads the rule param and round-trips an encoded id', () => {
		const url = new URL(`http://x${ruleHref('a b')}`);
		expect(highlightedRuleId(url.searchParams)).toBe('a b');
	});
	it('is null when absent or blank', () => {
		expect(highlightedRuleId(new URLSearchParams())).toBeNull();
		expect(highlightedRuleId(new URLSearchParams('rule=%20'))).toBeNull();
	});
});
