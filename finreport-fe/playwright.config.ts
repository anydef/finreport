import { defineConfig } from '@playwright/test';

// Smoke tests run against the dev server (not a prod build) in mock mode
// (§9 WP5: "never blocked on a running backend") on a non-default port so it
// doesn't collide with a `npm run dev` already running on 5173.
const PORT = 5174;

export default defineConfig({
	webServer: {
		command: `npm run dev -- --port ${PORT} --strictPort`,
		port: PORT,
		env: { PUBLIC_USE_MOCKS: '1' },
		reuseExistingServer: false
	},
	use: {
		baseURL: `http://localhost:${PORT}`
	},
	testDir: 'e2e'
});
