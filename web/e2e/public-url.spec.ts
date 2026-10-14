// Browser proof of ADR 0025: an installation reached by another address
// than its published one says where to go. All fixture data is invented.

import { expect, test } from './fixtures';
import { startServer } from './server';

const PUBLIC_URL = 'https://consolebook.example.invalid';

async function initialize(url: string, setupCode: string) {
	const response = await fetch(`${url}/api/setup`, {
		method: 'POST',
		headers: { 'content-type': 'application/json' },
		body: JSON.stringify({
			setup_code: setupCode,
			agency_name: 'Example County Communications',
			username: 'avery.admin',
			display_name: 'Avery Admin',
			password: 'invented-passphrase-1'
		})
	});
	expect(response.ok).toBe(true);
}

test('sign-in points to the published address when reached by another', async ({ page }) => {
	const server = await startServer({ serveArgs: ['--public-url', PUBLIC_URL] });
	try {
		await initialize(server.url, await server.setupCode());
		await page.goto(`${server.url}/login`);
		const note = page.getByRole('note');
		await expect(note).toContainText('meant to be opened at');
		await expect(note.getByRole('link', { name: PUBLIC_URL })).toHaveAttribute('href', PUBLIC_URL);
	} finally {
		await server.stop();
	}
});

test('sign-in shows no such note without a public URL', async ({ page, server, setupCode }) => {
	await initialize(server.url, setupCode);
	await page.goto('/login');
	await expect(page.getByLabel('Username')).toBeVisible();
	await expect(page.getByRole('note')).toHaveCount(0);
});
