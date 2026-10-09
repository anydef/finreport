/**
 * Which transaction row has its editor expanded. One at a time: opening
 * another row replaces the current one, so a long list never accumulates
 * open forms.
 */

/** Click on a row: collapse it if it is the open one, otherwise open it. */
export function toggleExpanded(current: string | null, id: string): string | null {
	return current === id ? null : id;
}

/** Keep the open row only while it is still in the list (new page, new filter). */
export function pruneExpanded(current: string | null, ids: string[]): string | null {
	return current !== null && ids.includes(current) ? current : null;
}
