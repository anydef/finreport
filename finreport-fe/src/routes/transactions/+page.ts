import { createGraphqlClient } from '$lib/graphqlClient';
import {
	defaultDateRange,
	defaultDateRangeFrom,
	type Account,
	type Transaction
} from '$lib/transactions';
import type { PageLoad } from './$types';

const DEFAULTS_QUERY = `
query TransactionsDefaultsQuery($accountId: String) {
	accounts { id accountId displayId accountType iban bic institute }
	lastTransactionDate(accountId: $accountId)
}
`;

const TRANSACTIONS_QUERY = `
query TransactionsQuery($startDate: String, $endDate: String, $accountId: String) {
	transactions(startDate: $startDate, endDate: $endDate, accountId: $accountId) {
		reference accountId bookingStatus bookingDate amount remitter deptor creditor creditorId creditorMandateId remittanceInfo transactionType
	}
}
`;

export const load: PageLoad = async ({ fetch, url }) => {
	const accountId = url.searchParams.get('accountId') || null;

	const client = createGraphqlClient(fetch);

	const defaultsResult = await client.query(DEFAULTS_QUERY, { accountId }).toPromise();

	if (defaultsResult.error) {
		const fallback = defaultDateRange(new Date());
		return {
			error: 'Failed to fetch accounts/date defaults from GraphQL API',
			startDate: url.searchParams.get('startDate') ?? fallback.start,
			endDate: url.searchParams.get('endDate') ?? fallback.end,
			accountId,
			accounts: [] as Account[],
			transactions: [] as Transaction[]
		};
	}

	const accounts = (defaultsResult.data?.accounts ?? []) as Account[];
	const defaults = defaultDateRangeFrom(
		defaultsResult.data?.lastTransactionDate ?? null,
		new Date()
	);
	const startDate = url.searchParams.get('startDate') ?? defaults.start;
	const endDate = url.searchParams.get('endDate') ?? defaults.end;

	const result = await client
		.query(TRANSACTIONS_QUERY, { startDate, endDate, accountId })
		.toPromise();

	if (result.error) {
		return {
			error: 'Failed to fetch transactions from GraphQL API',
			startDate,
			endDate,
			accountId,
			accounts,
			transactions: [] as Transaction[]
		};
	}

	return {
		error: null,
		startDate,
		endDate,
		accountId,
		accounts,
		transactions: (result.data?.transactions ?? []) as Transaction[]
	};
};
