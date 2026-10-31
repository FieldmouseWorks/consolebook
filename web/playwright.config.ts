import { defineConfig } from '@playwright/test';

// End-to-end proof against the real compiled server binary. Run
// `npm run build` and `cargo build` first; the spec starts the server
// itself on a scratch data directory.
// `cargo xtask proof` sets CONSOLEBOOK_E2E_REPORT to collect a JSON report
// alongside the usual output: its receipt requires proof that the suite ran.
const report = process.env.CONSOLEBOOK_E2E_REPORT;

export default defineConfig({
	testDir: './e2e',
	timeout: 60_000,
	reporter: report
		? [[process.env.CI ? 'dot' : 'list'], ['json', { outputFile: report }]]
		: undefined,
	use: {
		// A failure keeps its trace in test-results/, so an intermittent
		// failure brings its own evidence (#83).
		trace: 'retain-on-failure',
		// Prefer an environment-provided Chromium (CONSOLEBOOK_E2E_CHROMIUM)
		// over downloading a browser per Playwright version.
		launchOptions: process.env.CONSOLEBOOK_E2E_CHROMIUM
			? { executablePath: process.env.CONSOLEBOOK_E2E_CHROMIUM }
			: {}
	}
});
