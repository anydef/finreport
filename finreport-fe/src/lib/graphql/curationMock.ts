/**
 * Mock-mode only figures for the curation-progress card. The schema has no
 * "rules learned" field yet, so this is never read against a real backend
 * (the dashboard loader gates it on `PUBLIC_USE_MOCKS`).
 */
export const MOCK_RULES_LEARNED = 7;

/**
 * First-view baselines for mock mode, a little above the fixture's counts so
 * the "fewer" lines are visible without categorising anything.
 */
export const MOCK_CURATION_BASELINES = { uncategorized: 202, needsReview: 196 };
