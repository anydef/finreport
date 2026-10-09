/**
 * Pure helpers for the transaction detail modal: folding a mutation's
 * response back into the `Transaction` the list rendered, and the
 * "this will clear your splits" warning.
 */
import type {
	RecurringInfo,
	Transaction,
	TransactionLabel,
	TransactionSplit
} from './graphql/types';

type CategoryResult = {
	label: Pick<TransactionLabel, 'category' | 'source' | 'status'>;
	splits?: TransactionSplit[];
};

/**
 * `setTransactionCategory` writes the user-override layer, so the response
 * label is a resolved USER decision: the previous label's confidence, review
 * reason and LLM reasoning no longer describe it and are dropped. The
 * mutation also clears splits; an echo that omits them is read the same way.
 */
export function applyCategoryResult(tx: Transaction, result: CategoryResult): Transaction {
	return {
		...tx,
		label: {
			rule: null,
			confidence: null,
			reviewReason: null,
			proposedCategoryPath: null,
			reasoning: null,
			...result.label
		},
		splits: result.splits ?? []
	};
}

export function applyTagsResult(tx: Transaction, result: { tags: string[] }): Transaction {
	return { ...tx, tags: result.tags };
}

export function applyNoteResult(tx: Transaction, result: { note: string | null }): Transaction {
	return { ...tx, note: result.note };
}

/** Longest note the backend accepts, in characters (`MAX_NOTE_CHARS`). */
export const MAX_NOTE_LENGTH = 2000;

/** What a draft would be saved as: trimmed, and `null` when blank (which clears the note). */
export function noteToSave(draft: string): string | null {
	const trimmed = draft.trim();
	return trimmed === '' ? null : trimmed;
}

/** Whether saving `draft` would change the stored note. */
export function isNoteDirty(saved: string | null | undefined, draft: string): boolean {
	return noteToSave(draft) !== (saved?.trim() ? saved.trim() : null);
}

/** A message when `draft` cannot be saved, else `null`. */
export function noteProblem(draft: string): string | null {
	const length = [...draft.trim()].length;
	return length > MAX_NOTE_LENGTH
		? `A note can be at most ${MAX_NOTE_LENGTH} characters (this one is ${length}).`
		: null;
}

export function applyRecurringResult(
	tx: Transaction,
	result: { recurring: RecurringInfo }
): Transaction {
	return { ...tx, recurring: result.recurring };
}

/** Text to confirm before a category change, or `null` when nothing is lost. */
export function categoryChangeWarning(tx: Pick<Transaction, 'splits'>): string | null {
	const n = tx.splits?.length ?? 0;
	if (n === 0) return null;
	return `This transaction is split into ${n} parts. Changing its category removes the split.`;
}
