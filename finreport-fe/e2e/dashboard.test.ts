import { expect, test } from '@playwright/test';

// Smoke-tests the auth guard, login flow, dashboard rendering (charts +
// transactions) and logout against the mock backend (PUBLIC_USE_MOCKS=1,
// see playwright.config.ts) — no real GraphQL server required (§9 WP5).

test('redirects unauthenticated visitors to /login', async ({ page }) => {
	await page.goto('/');
	await expect(page).toHaveURL(/\/login/);
	await expect(page.getByRole('heading', { name: 'finreport' })).toBeVisible();
});

test('logs in and renders the dashboard from mocks', async ({ page }) => {
	await page.goto('/login');
	await page.getByLabel(/username/i).fill('demo');
	await page.getByLabel(/password/i).fill('demo');
	await page.getByRole('button', { name: /log in/i }).click();

	await expect(page).toHaveURL('/');
	await expect(page.getByText('Income', { exact: true })).toBeVisible();
	await expect(page.getByText('Spending', { exact: true })).toBeVisible();
	await expect(page.getByText('Net', { exact: true })).toBeVisible();
	await expect(page.getByRole('img', { name: /income and spending/i })).toBeVisible();
	await expect(page.getByRole('img', { name: /cash flow for/i })).toBeVisible();
	await expect(page.getByRole('table')).toBeVisible();

	await page.getByRole('link', { name: 'Transactions' }).click();
	await expect(page).toHaveURL(/\/transactions/);
	await expect(page.getByRole('table')).toBeVisible();

	await page.getByRole('button', { name: /log ?out/i }).click();
	await expect(page).toHaveURL(/\/login/);
});
