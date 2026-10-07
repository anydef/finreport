/**
 * Server-only GraphQL backend access, shared by the `/api/graphql` proxy
 * route, `hooks.server.ts` (per-request `me`) and the `/login` form action —
 * one place that knows how to reach the real backend *or* serve the WP0
 * mocks, so the three callers can't drift (§6, §9 WP5).
 *
 * `PUBLIC_USE_MOCKS=1` is the "never blocked on a running backend" switch
 * (§9 WP5): every GraphQL operation used by the frontend is served from the
 * mock fixtures in `src/lib/graphql/mocks/` plus a tiny in-memory session
 * stand-in for `me`/`login`/`logout`, with no network call at all.
 */
import { env as privateEnv } from '$env/dynamic/private';
import { env as publicEnv } from '$env/dynamic/public';
import type { RequestEvent } from '@sveltejs/kit';

import accountsMock from '$lib/graphql/mocks/accounts.json';
import cashflowSummaryMock from '$lib/graphql/mocks/cashflow-summary.json';
import cashflowGraphNetDeficitMock from '$lib/graphql/mocks/cashflow-graph-net-deficit.json';
import cashflowGraphTruncatedMock from '$lib/graphql/mocks/cashflow-graph-truncated.json';
import transactionsMock from '$lib/graphql/mocks/transactions.json';
import categoriesMock from '$lib/graphql/mocks/categories.json';
import categoryBreakdownMock from '$lib/graphql/mocks/category-breakdown.json';
import reviewQueueMock from '$lib/graphql/mocks/review-queue.json';
import rulesMock from '$lib/graphql/mocks/rules.json';
import transactionSplitMock from '$lib/graphql/mocks/transaction-split.json';

/** Name of the mock-mode session cookie, mirroring the real `fr_session` cookie's role. */
const MOCK_SESSION_COOKIE = 'fr_session';
const MOCK_USERNAME = 'demo';

export interface GraphqlRequestBody {
	query: string;
	variables?: Record<string, unknown>;
	operationName?: string | null;
}

export interface GraphqlBackendResult {
	status: number;
	/** JSON-serializable GraphQL response body (`{ data }` or `{ errors }`). */
	body: unknown;
	/** Raw `Set-Cookie` header values to relay onto the outer HTTP response. */
	setCookies: string[];
}

export function isMockMode(): boolean {
	return publicEnv.PUBLIC_USE_MOCKS === '1';
}

function backendUrl(): string {
	return privateEnv.GRAPHQL_URL || 'http://localhost:8080/graphql';
}

/** Extract the operation name out of `query CashflowGraph(...) { ... }`. */
function operationNameOf(body: GraphqlRequestBody): string | null {
	if (body.operationName) return body.operationName;
	const match = /\b(?:query|mutation)\s+(\w+)/.exec(body.query);
	return match ? match[1] : null;
}

function invalidCredentialsError() {
	return {
		errors: [
			{
				message: 'Invalid username or password',
				extensions: { code: 'INVALID_CREDENTIALS' }
			}
		]
	};
}

function mockMe(event: RequestEvent): unknown {
	const authed = event.cookies.get(MOCK_SESSION_COOKIE) === 'mock';
	return {
		data: {
			me: authed
				? {
						id: '00000000-0000-0000-0000-000000000001',
						username: MOCK_USERNAME,
						displayName: 'Demo User'
					}
				: null
		}
	};
}

function mockLogin(
	event: RequestEvent,
	variables: Record<string, unknown> | undefined
): GraphqlBackendResult {
	const input = (variables?.input ?? {}) as { username?: string; password?: string };
	if (!input.username || !input.password) {
		return { status: 200, body: invalidCredentialsError(), setCookies: [] };
	}
	// Mirrors the real cookie's shape (§4) closely enough for the proxy/hooks
	// relay path to be exercised end-to-end; the value itself is a fixed
	// sentinel, not a real session token.
	const setCookie = `${MOCK_SESSION_COOKIE}=mock; Path=/; HttpOnly; SameSite=Lax`;
	return {
		status: 200,
		body: {
			data: {
				login: {
					id: '00000000-0000-0000-0000-000000000001',
					username: input.username,
					displayName: 'Demo User'
				}
			}
		},
		setCookies: [setCookie]
	};
}

function mockLogout(): GraphqlBackendResult {
	const setCookie = `${MOCK_SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0`;
	return { status: 200, body: { data: { logout: true } }, setCookies: [setCookie] };
}

/** Pick the richer net/deficit cashflow-graph fixture unless the request is scoped to exactly one account. */
function mockCashflowGraph(variables: Record<string, unknown> | undefined): unknown {
	const accountIds = (variables?.filter as { accountIds?: unknown[] } | undefined)?.accountIds;
	if (Array.isArray(accountIds) && accountIds.length === 1) {
		return cashflowGraphTruncatedMock.data;
	}
	return cashflowGraphNetDeficitMock.data;
}

/**
 * `/admin/rules` + `/admin/categories` (§5, §10 WP6) mutations: the real
 * backend only publishes-then-upserts (§2.1, async via the labeler), so a
 * static fixture can't stand in the way `SplitTransaction`'s does. These
 * echo the caller's own input back as the "fresh row", which is enough for
 * the admin screens to round-trip against in mock mode.
 */
function findMockCategory(slug: string | undefined): {
	id: string;
	slug: string;
	name: string;
	kind: string;
} {
	const found = categoriesMock.data.categories.find((c) => c.slug === slug);
	return (
		found ?? {
			id: '00000000-0000-0000-0000-000000000000',
			slug: slug ?? '',
			name: slug ?? '',
			kind: 'EXPENSE'
		}
	);
}

function mockCreateOrUpdateRule(
	variables: Record<string, unknown> | undefined,
	existingId?: string
): unknown {
	const input = (variables?.input ?? {}) as {
		name?: string;
		categorySlug?: string;
		conditions?: unknown;
		priority?: number;
	};
	const existing = existingId ? rulesMock.data.rules.find((r) => r.id === existingId) : undefined;
	return {
		id: existingId ?? crypto.randomUUID(),
		name: input.name ?? existing?.name ?? 'Untitled rule',
		category: findMockCategory(input.categorySlug) ?? existing?.category,
		conditions: input.conditions ?? existing?.conditions ?? {},
		priority: input.priority ?? existing?.priority ?? 0,
		state: existing?.state ?? 'ACTIVE',
		origin: existing?.origin ?? 'USER',
		autoApproved: existing?.autoApproved ?? false,
		confidence: existing?.confidence ?? null,
		evidenceCount: existing?.evidenceCount ?? 0,
		createdAt: existing?.createdAt ?? new Date().toISOString()
	};
}

function mockSetRuleState(variables: Record<string, unknown> | undefined): unknown {
	const id = variables?.id as string | undefined;
	const state = variables?.state as string | undefined;
	const existing = rulesMock.data.rules.find((r) => r.id === id);
	return {
		...((existing ?? mockCreateOrUpdateRule(undefined, id)) as Record<string, unknown>),
		id,
		state: state ?? existing?.state ?? 'ACTIVE'
	};
}

/** `reapplyRule` returns how many transactions it will re-queue (§5); a fixed, plausible count in mock mode. */
function mockReapplyRule(): unknown {
	return 7;
}

function mockCreateOrUpdateCategory(
	variables: Record<string, unknown> | undefined,
	existingId?: string
): unknown {
	const input = (variables?.input ?? {}) as {
		slug?: string;
		name?: string;
		kind?: string;
		parentSlug?: string | null;
	};
	const parent = input.parentSlug ? findMockCategory(input.parentSlug) : undefined;
	return {
		id: existingId ?? crypto.randomUUID(),
		slug: input.slug ?? '',
		name: input.name ?? '',
		kind: input.kind ?? 'EXPENSE',
		parentId: parent?.id ?? null,
		depth: parent ? 2 : 1,
		archived: false,
		origin: 'user'
	};
}

function mockRenameCategory(variables: Record<string, unknown> | undefined): unknown {
	const id = variables?.id as string | undefined;
	const name = variables?.name as string | undefined;
	const existing = categoriesMock.data.categories.find((c) => c.id === id);
	return { id, slug: existing?.slug ?? '', name: name ?? existing?.name ?? '' };
}

function mockArchiveCategory(variables: Record<string, unknown> | undefined): unknown {
	const id = variables?.id as string | undefined;
	const existing = categoriesMock.data.categories.find((c) => c.id === id);
	return { id, slug: existing?.slug ?? '', archived: true };
}

function mockResponse(event: RequestEvent, body: GraphqlRequestBody): GraphqlBackendResult | null {
	switch (operationNameOf(body)) {
		case 'Me':
			return { status: 200, body: mockMe(event), setCookies: [] };
		case 'Login':
			return mockLogin(event, body.variables);
		case 'Logout':
			return mockLogout();
		case 'Accounts':
			return { status: 200, body: { data: accountsMock.data }, setCookies: [] };
		case 'Transactions':
			return { status: 200, body: { data: transactionsMock.data }, setCookies: [] };
		case 'CashflowSummary':
			return { status: 200, body: { data: cashflowSummaryMock.data }, setCookies: [] };
		case 'CashflowGraph':
			return { status: 200, body: { data: mockCashflowGraph(body.variables) }, setCookies: [] };
		// Iteration-2 (§5) stub operations: the WP0 mocks exist so WP5/WP6 can
		// build against a stable shape; resolvers on the real backend still
		// return NOT_IMPLEMENTED until their owning WP lands.
		case 'CategoryBreakdown':
			return { status: 200, body: { data: categoryBreakdownMock.data }, setCookies: [] };
		case 'ReviewQueue':
			return { status: 200, body: { data: reviewQueueMock.data }, setCookies: [] };
		case 'Rules':
			return { status: 200, body: { data: rulesMock.data }, setCookies: [] };
		case 'RecentlyAutoApprovedRules':
			// Field name differs from `Rules` (`recentlyAutoApprovedRules`, not
			// `rules`), and only auto-approved learned rules qualify (§2.8).
			return {
				status: 200,
				body: {
					data: {
						recentlyAutoApprovedRules: rulesMock.data.rules.filter(
							(r) => r.origin === 'LEARNED' && r.autoApproved
						)
					}
				},
				setCookies: []
			};
		case 'SplitTransaction':
			return { status: 200, body: { data: transactionSplitMock.data }, setCookies: [] };
		case 'Categories':
			return { status: 200, body: { data: categoriesMock.data }, setCookies: [] };
		case 'CreateRule':
			return {
				status: 200,
				body: { data: { createRule: mockCreateOrUpdateRule(body.variables) } },
				setCookies: []
			};
		case 'UpdateRule':
			return {
				status: 200,
				body: {
					data: { updateRule: mockCreateOrUpdateRule(body.variables, body.variables?.id as string) }
				},
				setCookies: []
			};
		case 'SetRuleState':
			return {
				status: 200,
				body: { data: { setRuleState: mockSetRuleState(body.variables) } },
				setCookies: []
			};
		case 'ReapplyRule':
			return { status: 200, body: { data: { reapplyRule: mockReapplyRule() } }, setCookies: [] };
		case 'CreateCategory':
			return {
				status: 200,
				body: { data: { createCategory: mockCreateOrUpdateCategory(body.variables) } },
				setCookies: []
			};
		case 'RenameCategory':
			return {
				status: 200,
				body: { data: { renameCategory: mockRenameCategory(body.variables) } },
				setCookies: []
			};
		case 'ArchiveCategory':
			return {
				status: 200,
				body: { data: { archiveCategory: mockArchiveCategory(body.variables) } },
				setCookies: []
			};
		default:
			return null;
	}
}

/**
 * Apply raw `Set-Cookie` strings from a `forwardGraphql` result onto the
 * current request's response via SvelteKit's `cookies` API (used by the
 * `/login` and `/logout` actions, which can't just copy the header the way
 * the `/api/graphql` route does — form actions don't expose a raw
 * `Response`). Parsing is intentionally minimal: the backend is the only
 * producer of these cookies and its shape is fixed by §4.
 */
export function relaySetCookies(event: RequestEvent, setCookies: string[]): void {
	for (const raw of setCookies) {
		const [pair, ...attrParts] = raw.split(';').map((part) => part.trim());
		const eq = pair.indexOf('=');
		if (eq === -1) continue;
		const name = pair.slice(0, eq);
		const value = pair.slice(eq + 1);
		const attrs: Record<string, string | boolean> = {};
		for (const attr of attrParts) {
			const [key, val] = attr.split('=').map((s) => s.trim());
			attrs[key.toLowerCase()] = val ?? true;
		}
		const maxAge = attrs['max-age'] ? Number(attrs['max-age']) : undefined;
		if (maxAge === 0) {
			event.cookies.delete(name, { path: (attrs['path'] as string) || '/' });
			continue;
		}
		event.cookies.set(name, value, {
			path: (attrs['path'] as string) || '/',
			httpOnly: Boolean(attrs['httponly']),
			secure: Boolean(attrs['secure']),
			sameSite: (attrs['samesite'] as 'lax' | 'strict' | 'none' | undefined)?.toLowerCase() as
				| 'lax'
				| 'strict'
				| 'none'
				| undefined,
			maxAge
		});
	}
}

function getSetCookies(headers: Headers): string[] {
	// `Headers.getSetCookie()` is the only correct way to read multiple
	// `Set-Cookie` values (`.get()` would merge them with commas, which is
	// not valid for cookies); fall back for runtimes without it.
	if (typeof headers.getSetCookie === 'function') return headers.getSetCookie();
	const single = headers.get('set-cookie');
	return single ? [single] : [];
}

/**
 * Forward a parsed GraphQL request to the real backend, or synthesize a
 * mock-mode response. Uses the event's own `fetch` so cookies/credentials
 * behave consistently with the rest of SvelteKit's server-side fetch
 * tracking.
 */
export async function forwardGraphql(
	event: RequestEvent,
	body: GraphqlRequestBody
): Promise<GraphqlBackendResult> {
	if (isMockMode()) {
		const mocked = mockResponse(event, body);
		if (mocked) return mocked;
		// An operation the mocks don't model (shouldn't happen for this
		// frontend's own queries) — fail loudly rather than guessing.
		return {
			status: 200,
			body: { errors: [{ message: `No mock for operation in: ${body.query.slice(0, 80)}` }] },
			setCookies: []
		};
	}

	const headers: Record<string, string> = { 'content-type': 'application/json' };
	const cookie = event.request.headers.get('cookie');
	if (cookie) headers['cookie'] = cookie;
	const origin = event.request.headers.get('origin');
	if (origin) headers['origin'] = origin;

	const upstream = await event.fetch(backendUrl(), {
		method: 'POST',
		headers,
		body: JSON.stringify(body)
	});
	const json = await upstream.json();
	return { status: upstream.status, body: json, setCookies: getSetCookies(upstream.headers) };
}
