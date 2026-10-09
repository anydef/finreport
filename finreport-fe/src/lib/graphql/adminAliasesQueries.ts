/**
 * GraphQL documents for `/admin/aliases`: the user's nicknames for merchants
 * and their own accounts. Display only; the bank's names are never changed.
 */

const DISPLAY_ALIAS_FIELDS = `
	kind
	key
	alias
	rawName
	transactionCount
	updatedAt`;

export const DISPLAY_ALIASES_QUERY = `
query DisplayAliases {
	displayAliases {${DISPLAY_ALIAS_FIELDS}
	}
}`;

export const SET_DISPLAY_ALIAS_MUTATION = `
mutation SetDisplayAlias($kind: DisplayAliasKind!, $key: String!, $alias: String!) {
	setDisplayAlias(kind: $kind, key: $key, alias: $alias) {${DISPLAY_ALIAS_FIELDS}
	}
}`;

export const REMOVE_DISPLAY_ALIAS_MUTATION = `
mutation RemoveDisplayAlias($kind: DisplayAliasKind!, $key: String!) {
	removeDisplayAlias(kind: $kind, key: $key)
}`;
