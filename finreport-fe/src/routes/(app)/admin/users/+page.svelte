<script lang="ts">
	/**
	 * `/admin/users`: MOCK-ONLY preview. Sample data from `usersMock.ts`; every
	 * action is inert and nothing is submitted anywhere. The backend does not
	 * serve user management yet (only the `user-admin` CLI does).
	 */
	import Badge from '$lib/components/Badge.svelte';
	import Button from '$lib/components/Button.svelte';
	import Card from '$lib/components/Card.svelte';
	import Field from '$lib/components/Field.svelte';
	import {
		actionAvailability,
		describeAction,
		summarizeUsers,
		toRows,
		validateCreateUser,
		type CreateUserErrors,
		type UserAction
	} from '$lib/adminUsersView';
	import { MOCK_ACCOUNTS, MOCK_USERS } from '$lib/graphql/usersMock';
	import type { PageData } from './$types';

	let { data }: { data: PageData } = $props();

	const rows = toRows(MOCK_USERS, MOCK_ACCOUNTS);
	const userById = new Map(MOCK_USERS.map((u) => [u.id, u]));
	const actions: { action: UserAction; label: string }[] = [
		{ action: 'set-password', label: 'Set password' },
		{ action: 'link', label: 'Link accounts' },
		{ action: 'unlink', label: 'Unlink' },
		{ action: 'promote', label: 'Make admin' },
		{ action: 'demote', label: 'Remove admin' },
		{ action: 'delete', label: 'Delete' }
	];
	const NOT_WIRED = 'Not wired up yet';

	let username = $state('');
	let password = $state('');
	let confirmation = $state('');
	let errors = $state<CreateUserErrors | null>(null);
	let checked = $state(false);

	function check(e: SubmitEvent) {
		e.preventDefault();
		const result = validateCreateUser(
			{ username, password, confirmation },
			MOCK_USERS.map((u) => u.username)
		);
		errors = result.errors;
		checked = result.ok;
	}
</script>

<div class="flex flex-col gap-6">
	<h1 class="text-lg font-semibold text-slate-900">Users</h1>

	{#if !data.mockMode}
		<Card>
			<p class="text-sm text-slate-600">
				User management is not available yet. For now, use the <code>user-admin</code> command line tool.
			</p>
		</Card>
	{:else}
		<p class="rounded-md bg-slate-100 px-3 py-2 text-sm text-slate-700" role="status">
			Preview with sample data. Nothing on this page is saved or sent anywhere, and every action is
			switched off until the backend exists.
		</p>

		<Card title="Users">
			<p class="mb-3 text-sm text-slate-600">{summarizeUsers(MOCK_USERS)}</p>
			<div class="overflow-x-auto">
				<table class="w-full text-left text-sm">
					<thead class="text-xs text-slate-500 uppercase">
						<tr>
							<th class="py-2 pr-4">User</th>
							<th class="py-2 pr-4">Role</th>
							<th class="py-2 pr-4">Accounts</th>
							<th class="py-2 pr-4">Last sign-in</th>
							<th class="py-2">Actions</th>
						</tr>
					</thead>
					<tbody class="divide-y divide-slate-100">
						{#each rows as row (row.id)}
							{@const user = userById.get(row.id)!}
							<tr class="align-top">
								<td class="py-2 pr-4">
									<div class="text-slate-800">{row.username}</div>
									{#if row.displayName}
										<div class="text-xs text-slate-500">{row.displayName}</div>
									{/if}
								</td>
								<td class="py-2 pr-4">
									<span class="flex flex-wrap gap-1">
										<Badge
											text={row.isAdmin ? 'Admin' : 'User'}
											variant={row.isAdmin ? 'info' : 'neutral'}
										/>
										{#if row.disabled}<Badge text="Disabled" variant="warning" />{/if}
									</span>
								</td>
								<td class="py-2 pr-4">
									<div class="text-slate-800">{row.linkSummary}</div>
									{#if row.linkedLabels.length > 0}
										<div class="text-xs text-slate-500">{row.linkedLabels.join(', ')}</div>
									{/if}
								</td>
								<td class="py-2 pr-4 text-slate-600">{row.lastLogin}</td>
								<td class="py-2">
									<div class="flex flex-wrap gap-1">
										{#each actions as { action, label } (action)}
											<Button variant="ghost" disabled title={NOT_WIRED} class="px-2 py-1 text-xs">
												{label}
											</Button>
										{/each}
									</div>
									<details class="mt-1 text-xs text-slate-500">
										<summary class="cursor-pointer">Confirm wording and availability</summary>
										<ul class="mt-1 flex flex-col gap-1">
											{#each actions as { action, label } (action)}
												{@const avail = actionAvailability(action, user, MOCK_USERS, MOCK_ACCOUNTS)}
												<li>
													<span class="font-medium">{label}:</span>
													{#if avail.available}
														{describeAction(
															action,
															user,
															MOCK_ACCOUNTS,
															user.accountIds[0]
														).lines.join(' ')}
													{:else}
														Unavailable. {avail.reason}
													{/if}
												</li>
											{/each}
										</ul>
									</details>
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
			<p class="mt-3 text-xs text-slate-500">
				Actions: {NOT_WIRED.toLowerCase()}. The buttons above do nothing.
			</p>
		</Card>

		<Card title="Create user">
			<form class="flex max-w-sm flex-col gap-3" onsubmit={check} novalidate>
				<Field label="Username" for="new-username">
					<input
						id="new-username"
						bind:value={username}
						autocomplete="off"
						class="rounded-md border border-slate-300 px-2 py-1.5 text-sm"
					/>
					{#each errors?.username ?? [] as message (message)}
						<span class="text-xs text-red-600" role="alert">{message}</span>
					{/each}
				</Field>
				<Field label="Password" for="new-password">
					<input
						id="new-password"
						type="password"
						bind:value={password}
						autocomplete="new-password"
						class="rounded-md border border-slate-300 px-2 py-1.5 text-sm"
					/>
					{#each errors?.password ?? [] as message (message)}
						<span class="text-xs text-red-600" role="alert">{message}</span>
					{/each}
				</Field>
				<Field label="Repeat password" for="new-password-confirm">
					<input
						id="new-password-confirm"
						type="password"
						bind:value={confirmation}
						autocomplete="new-password"
						class="rounded-md border border-slate-300 px-2 py-1.5 text-sm"
					/>
				</Field>
				<div class="flex items-center gap-3">
					<Button type="submit" variant="primary">Check</Button>
					{#if checked}
						<span class="text-sm text-slate-600" role="status">
							Looks valid. Nothing was created: this form is not wired up yet.
						</span>
					{/if}
				</div>
				<p class="text-xs text-slate-500">
					The server enforces no password rules today beyond what is checked here. Usernames are
					lowercased.
				</p>
			</form>
		</Card>
	{/if}
</div>
