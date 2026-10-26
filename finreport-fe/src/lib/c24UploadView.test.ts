import { describe, expect, it } from 'vitest';
import {
	bookingRange,
	c24Outcome,
	fileLabel,
	splitFiles,
	uploadErrorMessage
} from './c24UploadView';
import type { C24UploadResult } from './graphql/types';

const base: C24UploadResult = {
	files: [{ index: 0, rowCount: 10, error: null }],
	newTransactions: 10,
	unchangedTransactions: 0,
	updatedTransactions: 0,
	newAccounts: 1,
	updatedAccounts: 0,
	accountCount: 1,
	earliestBookingDate: '2026-03-02',
	latestBookingDate: '2026-10-28'
};

describe('c24Outcome', () => {
	it('reports a clean import', () => {
		const o = c24Outcome(base);
		expect(o.tone).toBe('good');
		expect(o.lines[0]).toBe('Read 10 rows from 1 file, booked 2 Mar 2026 to 28 Oct 2026.');
		expect(o.lines[1]).toBe('10 new transactions.');
		expect(o.lines[2]).toBe('1 account (1 new).');
		expect(o.rejected).toEqual([]);
	});

	it('uses singular for counts of exactly 1', () => {
		const o = c24Outcome({
			...base,
			files: [{ index: 0, rowCount: 1, error: null }],
			newTransactions: 1,
			latestBookingDate: '2026-03-02'
		});
		expect(o.lines[0]).toBe('Read 1 row from 1 file, booked 2 Mar 2026.');
		expect(o.lines[1]).toBe('1 new transaction.');
	});

	it('treats a re-upload that changed nothing as good, not an error', () => {
		const o = c24Outcome({
			...base,
			newTransactions: 0,
			unchangedTransactions: 10,
			newAccounts: 0
		});
		expect(o.tone).toBe('good');
		expect(o.lines[1]).toBe('Nothing changed: everything in these files was already imported.');
		expect(o.lines[2]).toBe('1 account.');
	});

	it('mixes new, updated and already-imported counts', () => {
		const o = c24Outcome({
			...base,
			newTransactions: 3,
			updatedTransactions: 2,
			unchangedTransactions: 5,
			updatedAccounts: 1,
			accountCount: 2
		});
		expect(o.lines[1]).toBe('3 new transactions, 2 updated, 5 already imported.');
		expect(o.lines[2]).toBe('2 accounts (1 new, 1 updated).');
	});

	it('is a warning when some files are rejected, naming them verbatim', () => {
		const o = c24Outcome(
			{
				...base,
				files: [
					{ index: 0, rowCount: 4, error: null },
					{ index: 1, rowCount: 0, error: 'column "Betrag", line 7: not an amount' },
					{ index: 2, rowCount: 6, error: null }
				]
			},
			['a.csv', 'b.csv', 'c.csv']
		);
		expect(o.tone).toBe('warn');
		expect(o.readCount).toBe(2);
		expect(o.rowTotal).toBe(10);
		expect(o.rejected).toEqual([
			{ label: 'File 2 (b.csv)', message: 'column "Betrag", line 7: not an amount' }
		]);
		expect(o.lines.at(-1)).toBe('1 file was rejected whole and not imported; the rest were.');
	});

	it('is bad when every file is rejected', () => {
		const o = c24Outcome({
			...base,
			files: [
				{ index: 0, rowCount: 0, error: 'bad header' },
				{ index: 1, rowCount: 0, error: 'bad header' }
			],
			newTransactions: 0,
			earliestBookingDate: null,
			latestBookingDate: null
		});
		expect(o.tone).toBe('bad');
		expect(o.lines).toEqual(['Every file was rejected, so nothing was imported.']);
		expect(o.rejected).toHaveLength(2);
	});

	it('says "the file" for a single rejected file', () => {
		const o = c24Outcome({ ...base, files: [{ index: 0, rowCount: 0, error: 'bad header' }] });
		expect(o.lines[0]).toBe('The file was rejected, so nothing was imported.');
	});

	it('handles zero files', () => {
		const o = c24Outcome({ ...base, files: [] });
		expect(o.tone).toBe('bad');
		expect(o.lines).toEqual(['No files were uploaded.']);
	});

	it('handles readable files with no rows', () => {
		const o = c24Outcome({
			...base,
			files: [{ index: 0, rowCount: 0, error: null }],
			newTransactions: 0,
			newAccounts: 0,
			accountCount: 0,
			earliestBookingDate: null,
			latestBookingDate: null
		});
		expect(o.lines[0]).toBe('Read 0 rows from 1 file.');
		expect(o.lines[1]).toBe('Nothing to import: the files contain no transactions.');
	});
});

describe('helpers', () => {
	it('bookingRange', () => {
		expect(bookingRange(null, null)).toBeNull();
		expect(bookingRange('2026-03-02', null)).toBe('2 Mar 2026');
		expect(bookingRange('2026-03-02', '2026-03-05')).toBe('2 Mar 2026 to 5 Mar 2026');
	});
	it('fileLabel is 1-based and tolerates missing names', () => {
		expect(fileLabel(0)).toBe('File 1');
		expect(fileLabel(2, ['a', 'b'])).toBe('File 3');
	});
	it('splitFiles', () => {
		const s = splitFiles([
			{ index: 0, rowCount: 1, error: null },
			{ index: 1, rowCount: 0, error: 'x' }
		]);
		expect(s.accepted).toHaveLength(1);
		expect(s.rejected[0].index).toBe(1);
	});
	it('explains both Kafka codes and falls back', () => {
		expect(uploadErrorMessage('KAFKA_UNAVAILABLE', 'x')).toMatch(/not configured/);
		expect(uploadErrorMessage('KAFKA_PUBLISH_FAILED', 'x')).toMatch(/Upload the same files again/);
		expect(uploadErrorMessage(undefined, 'no files given')).toBe(
			'Could not upload the files: no files given'
		);
		expect(uploadErrorMessage(undefined, undefined)).toBe('Could not upload the files. Try again.');
	});
});
