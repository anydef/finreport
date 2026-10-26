/**
 * Page shaping for `/admin/c24`. Pure, so the wording and the edge cases are
 * unit-tested. Nothing here ever sees file text: only the server's counts and
 * its value-free rejection messages, which are shown verbatim.
 */
import { formatDisplayDate } from '$lib/format';
import type { C24FileResult, C24UploadResult } from '$lib/graphql/types';

export type Tone = 'good' | 'warn' | 'bad';

export interface C24Outcome {
	tone: Tone;
	/** Headline plus detail lines, ready to render. */
	lines: string[];
	rejected: { label: string; message: string }[];
	/** Files read successfully. */
	readCount: number;
	/** Rows read across the readable files. */
	rowTotal: number;
}

function plural(n: number, one: string, many: string): string {
	return `${n} ${n === 1 ? one : many}`;
}

export function splitFiles(files: C24FileResult[]): {
	accepted: C24FileResult[];
	rejected: C24FileResult[];
} {
	return {
		accepted: files.filter((f) => !f.error),
		rejected: files.filter((f) => f.error)
	};
}

/** "2 Mar 2026 to 28 Sep 2026", a single date when both ends match, else null. */
export function bookingRange(earliest: string | null, latest: string | null): string | null {
	if (!earliest && !latest) return null;
	const a = (earliest ?? latest) as string;
	const b = (latest ?? earliest) as string;
	return a === b ? formatDisplayDate(a) : `${formatDisplayDate(a)} to ${formatDisplayDate(b)}`;
}

/** Position is 1-based for people; the server's index starts at 0. */
export function fileLabel(index: number, names?: string[]): string {
	const name = names?.[index];
	return name ? `File ${index + 1} (${name})` : `File ${index + 1}`;
}

export function c24Outcome(result: C24UploadResult, names?: string[]): C24Outcome {
	const { accepted, rejected } = splitFiles(result.files);
	const rowTotal = accepted.reduce((sum, f) => sum + f.rowCount, 0);
	const changed = result.newTransactions + result.updatedTransactions;
	const lines: string[] = [];

	if (accepted.length > 0) {
		const readLine = `Read ${plural(rowTotal, 'row', 'rows')} from ${plural(accepted.length, 'file', 'files')}`;
		const range = bookingRange(result.earliestBookingDate, result.latestBookingDate);
		lines.push(range ? `${readLine}, booked ${range}.` : `${readLine}.`);

		if (changed === 0) {
			lines.push(
				result.unchangedTransactions > 0
					? 'Nothing changed: everything in these files was already imported.'
					: 'Nothing to import: the files contain no transactions.'
			);
		} else {
			const bits = [plural(result.newTransactions, 'new transaction', 'new transactions')];
			if (result.updatedTransactions > 0) bits.push(`${result.updatedTransactions} updated`);
			if (result.unchangedTransactions > 0)
				bits.push(`${result.unchangedTransactions} already imported`);
			lines.push(bits.join(', ') + '.');
		}

		const accountBits: string[] = [];
		if (result.newAccounts > 0) accountBits.push(`${result.newAccounts} new`);
		if (result.updatedAccounts > 0) accountBits.push(`${result.updatedAccounts} updated`);
		const accounts = plural(result.accountCount, 'account', 'accounts');
		lines.push(
			accountBits.length > 0 ? `${accounts} (${accountBits.join(', ')}).` : `${accounts}.`
		);
	}

	if (accepted.length === 0) {
		lines.push(
			result.files.length === 0
				? 'No files were uploaded.'
				: `${result.files.length === 1 ? 'The file was' : 'Every file was'} rejected, so nothing was imported.`
		);
	} else if (rejected.length > 0) {
		lines.push(
			`${plural(rejected.length, 'file was', 'files were')} rejected whole and not imported; the rest were.`
		);
	}

	const tone: Tone = accepted.length === 0 ? 'bad' : rejected.length === 0 ? 'good' : 'warn';
	return {
		tone,
		lines,
		rejected: rejected.map((f) => ({
			label: fileLabel(f.index, names),
			message: f.error as string
		})),
		readCount: accepted.length,
		rowTotal
	};
}

/**
 * User-facing text for a failed upload. The server's own messages are
 * value-free, but only the known codes are explained; anything else gets a
 * generic line plus the server's message.
 */
export function uploadErrorMessage(code: string | undefined, message: string | undefined): string {
	if (code === 'KAFKA_UNAVAILABLE') {
		return 'The event log (Kafka) is not configured on this backend, so nothing could be imported. `just dev-be` runs without a broker; use the full local stack (`just dev-up`) to import.';
	}
	if (code === 'KAFKA_PUBLISH_FAILED') {
		return 'The event log did not accept the upload, so some or all rows may not be stored. Upload the same files again: rows already stored are skipped.';
	}
	return message
		? `Could not upload the files: ${message}`
		: 'Could not upload the files. Try again.';
}
