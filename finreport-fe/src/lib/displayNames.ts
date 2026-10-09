/**
 * Display-name resolution for counterparties and accounts. The backend
 * resolves the user's nicknames (`Transaction.counterpartyDisplayName`,
 * `Account.displayName`); these helpers pick the right field and keep the raw
 * bank value reachable as evidence. Raw values stay in the data model.
 */

import type { Account, Transaction } from './graphql/types';

type CounterpartyFields = Pick<Transaction, 'counterpartyName'> & {
	counterpartyDisplayName?: string | null;
};

/** The name to show for a transaction's counterparty; `null` when it has none. */
export function counterpartyLabel(tx: CounterpartyFields): string | null {
	return tx.counterpartyDisplayName ?? tx.counterpartyName ?? null;
}

/**
 * The bank's own counterparty string when a nickname is hiding it, else `null`
 * (nothing to add when the display name already is the bank name).
 */
export function rawCounterpartyName(tx: CounterpartyFields): string | null {
	const raw = tx.counterpartyName;
	if (!raw) return null;
	return tx.counterpartyDisplayName && tx.counterpartyDisplayName !== raw ? raw : null;
}

/** The name to show for an account: the nickname, else the login label, IBAN or ids. */
export function accountLabel(
	a: Pick<Account, 'id' | 'label' | 'displayId'> & { displayName?: string | null }
): string {
	return a.displayName ?? a.label ?? a.displayId ?? a.id;
}

/**
 * Account-node labels in the Sankey come from the login label server-side;
 * swap in each account's display name, matched on the node's `refId`.
 */
export function relabelAccountNodes<
	N extends { label: string; refType: string | null; refId: string | null }
>(nodes: N[], accounts: Pick<Account, 'id' | 'displayName'>[]): N[] {
	const names = new Map(accounts.map((a) => [a.id, a.displayName]));
	return nodes.map((n) => {
		const name = n.refType === 'account' && n.refId ? names.get(n.refId) : undefined;
		return name ? { ...n, label: name } : n;
	});
}
