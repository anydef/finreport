/**
 * Mock for `/admin/c24`, served from `graphqlBackend.ts` in mock mode.
 *
 * Deliberately not the happy path: three files, one rejected with the parser's
 * wording, a mix of new / unchanged / updated rows, and two accounts. Counts
 * and dates are invented. The file text is never inspected beyond its count,
 * and never echoed.
 */
import type { C24UploadResult } from './types';

export function mockUploadC24Transactions(variables: Record<string, unknown> | undefined): {
	uploadC24Transactions: C24UploadResult;
} {
	const count = Array.isArray(variables?.files) ? variables.files.length : 0;
	const canned = [
		{ index: 0, rowCount: 42, error: null },
		{ index: 1, rowCount: 0, error: 'column "Betrag", line 7: not an amount' },
		{ index: 2, rowCount: 17, error: null }
	];
	const files = Array.from({ length: count }, (_, i) =>
		i < canned.length ? canned[i] : { index: i, rowCount: 5, error: null }
	);
	const anyRead = files.some((f) => !f.error);
	return {
		uploadC24Transactions: {
			files,
			newTransactions: anyRead ? 31 : 0,
			unchangedTransactions: anyRead ? 24 : 0,
			updatedTransactions: anyRead ? 4 : 0,
			newAccounts: anyRead ? 1 : 0,
			updatedAccounts: anyRead ? 1 : 0,
			accountCount: anyRead ? 2 : 0,
			earliestBookingDate: anyRead ? '2026-03-02' : null,
			latestBookingDate: anyRead ? '2026-09-28' : null
		}
	};
}
