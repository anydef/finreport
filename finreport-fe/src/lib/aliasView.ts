/**
 * Pure helpers for the display-alias admin screen (`/admin/aliases`): the
 * user's nicknames for merchants and for their own accounts. Mirrors the
 * server's limits (`graphql/display_aliases.rs`) so the form can say no before
 * a round trip; the server stays the authority.
 */
import type { DisplayAlias, DisplayAliasKind } from '$lib/graphql/types';

export const MAX_ALIAS_LENGTH = 80;

/** A reason the alias cannot be saved, or `null` when it can. */
export function aliasError(input: string): string | null {
	const alias = input.trim();
	if (alias === '') return 'Enter a nickname.';
	if ([...alias].length > MAX_ALIAS_LENGTH) {
		return `Keep it to ${MAX_ALIAS_LENGTH} characters or fewer.`;
	}
	// eslint-disable-next-line no-control-regex
	if (/[\u0000-\u001f\u007f]/.test(alias)) return 'Use plain text only.';
	return null;
}

export function kindLabel(kind: DisplayAliasKind): string {
	return kind === 'ACCOUNT' ? 'My account' : 'Merchant';
}

/** Alphabetical by nickname, case-insensitive; stable for equal names. */
export function sortAliases(aliases: DisplayAlias[]): DisplayAlias[] {
	return [...aliases].sort((a, b) =>
		a.alias.localeCompare(b.alias, undefined, { sensitivity: 'base' })
	);
}

/** "Bank name: X" plus the transaction count for merchants. */
export function describeAliasTarget(alias: DisplayAlias): string {
	const base = `Bank name: ${alias.rawName}`;
	if (alias.transactionCount === null) return base;
	const n = alias.transactionCount;
	return `${base} · ${n} ${n === 1 ? 'transaction' : 'transactions'}`;
}

/** How a connected account is offered in the "add" picker. */
export function accountOptionLabel(account: {
	label: string | null;
	iban: string | null;
	displayId: string | null;
	id: string;
}): string {
	const name = account.label?.trim() || null;
	const iban = account.iban?.trim() || null;
	if (name && iban) return `${name} (${iban})`;
	return name ?? iban ?? account.displayId ?? account.id;
}
