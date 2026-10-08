/**
 * How a split transaction presents itself in a list row. A split's amount is
 * the whole, but every total counts it by its parts (iteration 4 §3.1), so the
 * row has to say it is a split; its own `label.category` is `null`, which would
 * otherwise read as "uncategorised".
 */

import type { TransactionSplit } from './graphql/types';

export interface SplitIndicator {
	/** Compact pill text, e.g. "split · 3 parts". */
	text: string;
	/** Screen-reader text for the expand/collapse button. */
	ariaLabel: string;
	count: number;
}

/** `null` for a plain (unsplit) transaction. */
export function splitIndicator(splits: TransactionSplit[]): SplitIndicator | null {
	const count = splits.length;
	if (count === 0) return null;
	const noun = count === 1 ? 'part' : 'parts';
	return {
		text: `split · ${count} ${noun}`,
		ariaLabel: `Show the ${count} ${noun} of this split transaction`,
		count
	};
}
