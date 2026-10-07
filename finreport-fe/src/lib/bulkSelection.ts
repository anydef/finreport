/**
 * Selection model for bulk-editing a transaction table.
 *
 * A selection is either an explicit set of ids, or "everything matching the
 * table's current filter". The second form is what the header checkbox
 * produces: the table is paged, so "all" must not mean "the 50 rows loaded".
 * Both map onto a `TransactionFilter` (`selectionFilter`), which is what the
 * bulk mutations take, so the server applies them to exactly what was meant.
 *
 * "All matching" cannot express exceptions (the contract has no exclusion
 * field), so rows are not individually untickable in that mode; clearing the
 * selection is the way back.
 */
import type { BulkEditResult, Transaction, TransactionFilter } from './graphql/types';

export type Selection =
	| { mode: 'ids'; ids: string[]; withSplits: string[] }
	| { mode: 'all-matching' };

export const EMPTY_SELECTION: Selection = { mode: 'ids', ids: [], withSplits: [] };

export function selectAllMatching(): Selection {
	return { mode: 'all-matching' };
}

export function isSelected(sel: Selection, id: string): boolean {
	return sel.mode === 'all-matching' || sel.ids.includes(id);
}

/** Tick or untick one row. No-op in all-matching mode (see the module note). */
export function toggleRow(sel: Selection, tx: Pick<Transaction, 'id' | 'splits'>): Selection {
	if (sel.mode === 'all-matching') return sel;
	if (sel.ids.includes(tx.id)) {
		return {
			mode: 'ids',
			ids: sel.ids.filter((id) => id !== tx.id),
			withSplits: sel.withSplits.filter((id) => id !== tx.id)
		};
	}
	return {
		mode: 'ids',
		ids: [...sel.ids, tx.id],
		withSplits: tx.splits.length > 0 ? [...sel.withSplits, tx.id] : sel.withSplits
	};
}

/** Exact number of transactions the selection covers. */
export function selectedCount(sel: Selection, totalCount: number): number {
	return sel.mode === 'all-matching' ? totalCount : sel.ids.length;
}

export type HeaderState = 'none' | 'some' | 'all';

export function headerState(sel: Selection): HeaderState {
	if (sel.mode === 'all-matching') return 'all';
	return sel.ids.length > 0 ? 'some' : 'none';
}

/** Header checkbox: anything selected -> clear; nothing selected -> all matching. */
export function toggleHeader(sel: Selection): Selection {
	return headerState(sel) === 'none' ? selectAllMatching() : EMPTY_SELECTION;
}

/**
 * The filter a bulk mutation should run with: the table's own filter, narrowed
 * to the ticked ids when the selection is explicit.
 */
export function selectionFilter(base: TransactionFilter, sel: Selection): TransactionFilter {
	return sel.mode === 'all-matching' ? { ...base } : { ...base, transactionIds: [...sel.ids] };
}

/**
 * Selected transactions known to have splits. In all-matching mode only the
 * loaded rows can be inspected, so the number is a lower bound (`exact: false`);
 * the authoritative figure is the server's `splitsCleared`.
 */
export function splitCount(
	sel: Selection,
	loadedRows: Pick<Transaction, 'splits'>[]
): { count: number; exact: boolean } {
	if (sel.mode === 'all-matching') {
		return { count: loadedRows.filter((r) => r.splits.length > 0).length, exact: false };
	}
	return { count: sel.withSplits.length, exact: true };
}

/**
 * Stable identity of a filter, key order and undefined/empty values ignored.
 * A selection is only valid for the key it was made under.
 */
export function filterKey(filter: TransactionFilter): string {
	const entries = Object.entries(filter as Record<string, unknown>)
		.filter(([, v]) => v !== undefined && v !== null && !(Array.isArray(v) && v.length === 0))
		.sort(([a], [b]) => a.localeCompare(b));
	return JSON.stringify(entries);
}

export type OutcomeTone = 'success' | 'partial' | 'failed';

export interface Outcome {
	tone: OutcomeTone;
	headline: string;
	details: string[];
}

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? '' : 's'}`;

/** Turn a result into something a partial failure cannot be mistaken for success. */
export function describeResult(result: BulkEditResult): Outcome {
	const details: string[] = [];
	if (result.splitsCleared > 0) {
		details.push(`${plural(result.splitsCleared, 'split definition')} discarded.`);
	}
	if (result.failed > 0 && result.applied === 0) {
		return {
			tone: 'failed',
			headline: `Nothing was changed: ${result.failed} of ${result.matched} failed.`,
			details
		};
	}
	if (result.failed > 0) {
		details.unshift('The operation is not atomic. Repeat the edit to retry the failed ones.');
		return {
			tone: 'partial',
			headline: `Partly applied: ${result.applied} changed, ${result.failed} failed (of ${result.matched}).`,
			details
		};
	}
	return {
		tone: 'success',
		headline: `${plural(result.applied, 'transaction')} updated (of ${result.matched} matched).`,
		details
	};
}
