import { describe, expect, it } from 'vitest';
import { labelSourceBadge, needsReviewBadge } from './labelBadge';
import type { TransactionLabel } from './graphql/types';

function label(partial: Partial<TransactionLabel>): TransactionLabel {
	return {
		category: null,
		source: 'USER',
		rule: null,
		confidence: null,
		status: 'RESOLVED',
		reviewReason: null,
		proposedCategoryPath: null,
		reasoning: null,
		...partial
	};
}

describe('labelSourceBadge', () => {
	it('returns null for no label at all', () => {
		expect(labelSourceBadge(null)).toBeNull();
	});

	it('badges a user-set label', () => {
		expect(labelSourceBadge(label({ source: 'USER' }))).toEqual({
			text: 'you',
			variant: 'neutral'
		});
	});

	it('badges a plain (non-auto-approved) rule label', () => {
		expect(labelSourceBadge(label({ source: 'RULE', confidence: 0.5 }))).toEqual({
			text: 'rule',
			variant: 'info'
		});
	});

	it('badges a learned rule at/above the 0.9 auto-approve threshold as auto-rule', () => {
		expect(labelSourceBadge(label({ source: 'RULE', confidence: 0.9 }))).toEqual({
			text: 'auto-rule',
			variant: 'success'
		});
		expect(labelSourceBadge(label({ source: 'RULE', confidence: 0.95 }))).toEqual({
			text: 'auto-rule',
			variant: 'success'
		});
	});

	it('badges a rule label with no confidence as a plain rule', () => {
		expect(labelSourceBadge(label({ source: 'RULE', confidence: null }))).toEqual({
			text: 'rule',
			variant: 'info'
		});
	});

	it('badges an llm-cache label', () => {
		expect(labelSourceBadge(label({ source: 'LLM_CACHE' }))).toEqual({
			text: 'cached',
			variant: 'info'
		});
	});

	it('badges an llm label', () => {
		expect(labelSourceBadge(label({ source: 'LLM' }))).toEqual({ text: 'AI', variant: 'warning' });
	});
});

describe('needsReviewBadge', () => {
	it('returns null when resolved', () => {
		expect(needsReviewBadge(label({ status: 'RESOLVED' }))).toBeNull();
	});

	it('returns null when there is no label at all', () => {
		expect(needsReviewBadge(null)).toBeNull();
	});

	it('badges a held label', () => {
		expect(needsReviewBadge(label({ status: 'NEEDS_REVIEW' }))).toEqual({
			text: 'needs review',
			variant: 'warning'
		});
	});
});
