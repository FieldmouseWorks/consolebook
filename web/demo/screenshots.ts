// `npm run screenshots`: seeds a scratch demo installation and captures the
// README's screenshots into docs/screenshots/. Build the interface and the
// server first, as for `npm run demo`. Set CONSOLEBOOK_E2E_CHROMIUM to use a
// local Chromium, as the browser tests do.

import { join } from 'node:path';
import { chromium, type Browser, type Page } from '@playwright/test';
import { startServer } from '../e2e/server.ts';
import { DEMO_PASSWORD, PEOPLE, seedDemo } from './seed.ts';

const OUT = join(import.meta.dirname, '../../docs/screenshots');

async function signedIn(browser: Browser, base: string, person: keyof typeof PEOPLE): Promise<Page> {
	const context = await browser.newContext({
		baseURL: base,
		viewport: { width: 1280, height: 900 },
		deviceScaleFactor: 2,
		colorScheme: 'light',
		locale: 'en-US',
		timezoneId: 'America/Los_Angeles'
	});
	const page = await context.newPage();
	await page.goto('/login');
	await page.getByLabel('Username').fill(PEOPLE[person].username);
	await page.getByLabel('Password').fill(DEMO_PASSWORD);
	await page.getByRole('button', { name: 'Sign in' }).click();
	await page.waitForURL((url) => url.pathname === '/');
	return page;
}

/** The top of the page as a person first sees it, or one panel of it. */
async function shoot(page: Page, path: string, name: string, heading: string, panel = false) {
	await page.goto(path);
	const title = page.getByRole('heading', { name: heading, exact: true }).first();
	await title.waitFor();
	const file = join(OUT, `${name}.png`);
	if (panel) await page.locator('section', { has: title }).last().screenshot({ path: file });
	else await page.screenshot({ path: file });
	console.log(`docs/screenshots/${name}.png`);
}

const server = await startServer();
const executablePath = process.env.CONSOLEBOOK_E2E_CHROMIUM;
const browser = await chromium.launch(executablePath ? { executablePath } : {});
try {
	const seeded = await seedDemo(server.url, await server.setupCode());

	const jordan = await signedIn(browser, server.url, 'jordan');
	await shoot(jordan, `/drafts/${seeded.openDraft}`, 'daily-report', 'Daily Observation Report');

	const casey = await signedIn(browser, server.url, 'coordinator');
	await shoot(casey, `/enrollments/${seeded.taylorEnrollment}`, 'task-signoffs', 'Task signoffs', true);
	await shoot(casey, `/drafts/${seeded.amendedRecord}`, 'version-history', 'Version history', true);

	const taylor = await signedIn(browser, server.url, 'taylor');
	await shoot(taylor, '/records', 'trainee-records', 'My records');
} finally {
	await browser.close();
	await server.stop();
}
