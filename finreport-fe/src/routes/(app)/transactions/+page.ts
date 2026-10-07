import { createGraphqlClient } from '$lib/graphqlClient';
import {
	ACCOUNTS_QUERY,
	CATEGORIES_QUERY,
	TAGS_QUERY,
	TRANSACTIONS_QUERY
} from '$lib/graphql/queries';
import type { Account, Category, TagCount, TransactionPage } from '$lib/graphql/types';
import { expandSelectedSlugs } from '$lib/categoryTree';
import {
	defaultGranularity,
	presetRange,
	toDateInputValue,
	type DateRange,
	type PeriodPresetId
} from '$lib/period';
import type { PageLoad } from './$types';

const PAGE_LIMIT = 50;

function readRange(url: URL, preset: PeriodPresetId, today: Date): DateRange {
	if (preset === 'custom') {
		const fallback = presetRange('this-month', today);
		return {
			start: url.searchParams.get('start') ?? fallback.start,
			end: url.searchParams.get('end') ?? fallback.end
		};
	}
	return presetRange(preset, today);
}

export const load: PageLoad = async ({ fetch, url }) => {
	const today = new Date();
	const preset = (url.searchParams.get('preset') as PeriodPresetId | null) ?? 'this-month';
	const range = readRange(url, preset, today);
	const offset = Number(url.searchParams.get('offset') ?? '0') || 0;
	const accountId = url.searchParams.get('accountId') ?? undefined;
	const search = url.searchParams.get('search') ?? undefined;
	const categorySlugs = url.searchParams.get('categorySlugs')?.split(',').filter(Boolean) ?? [];
	const uncategorized = url.searchParams.get('uncategorized') === 'true';
	const needsReview = url.searchParams.get('needsReview') === 'true';
	// Tri-state filters (§5): param absent = either, 'true'/'false' = that value.
	const tags = url.searchParams.get('tags')?.split(',').filter(Boolean) ?? [];
	const recurringParam = url.searchParams.get('recurring');
	const recurring =
		recurringParam === 'true' ? true : recurringParam === 'false' ? false : undefined;
	const transferParam = url.searchParams.get('transfer');
	const transfer = transferParam === 'true' ? true : transferParam === 'false' ? false : undefined;

	const client = createGraphqlClient(fetch);

	const [accountsResult, categoriesResult, tagsResult] = await Promise.all([
		client.query(ACCOUNTS_QUERY, {}).toPromise(),
		client.query(CATEGORIES_QUERY, {}).toPromise(),
		client.query(TAGS_QUERY, {}).toPromise()
	]);
	const categories = (categoriesResult.data?.categories ?? []) as Category[];

	const filter = {
		startDate: range.start,
		endDate: range.end,
		accountIds: accountId ? [accountId] : undefined,
		search: search || undefined,
		categorySlugs: categorySlugs.length
			? expandSelectedSlugs(categorySlugs, categories)
			: undefined,
		uncategorized: uncategorized || undefined,
		needsReview: needsReview || undefined,
		tags: tags.length ? tags : undefined,
		recurring,
		transfer
	};

	const transactionsResult = await client
		.query(TRANSACTIONS_QUERY, { filter, page: { limit: PAGE_LIMIT, offset } })
		.toPromise();

	return {
		preset,
		start: range.start,
		end: range.end,
		granularity: defaultGranularity(range),
		accountId,
		search,
		categorySlugs,
		uncategorized,
		needsReview,
		tags,
		recurring,
		transfer,
		offset,
		error: Boolean(
			accountsResult.error || categoriesResult.error || tagsResult.error || transactionsResult.error
		),
		accounts: (accountsResult.data?.accounts ?? []) as Account[],
		categories,
		allTags: (tagsResult.data?.tags ?? []) as TagCount[],
		transactionFilter: filter,
		transactions: transactionsResult.data?.transactions as TransactionPage | undefined,
		today: toDateInputValue(today)
	};
};
