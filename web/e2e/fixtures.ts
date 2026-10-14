import { test as base, expect, type Page } from '@playwright/test';
import { startServer } from './server';

export { expect };

/**
 * Signs out and waits until the server has ended the session. The shell
 * keeps a signed-in browser out of /login and /reset, so navigating on
 * before the logout completes races that redirect.
 */
export async function signOut(page: Page): Promise<void> {
	await page.getByRole('button', { name: 'Sign out' }).click();
	await expect(page).toHaveURL(/\/login$/);
}

export const test = base.extend<{
	server: Awaited<ReturnType<typeof startServer>>;
	setupCode: string;
}>({
	server: async ({}, use) => {
		const server = await startServer();
		try {
			await use(server);
		} finally {
			// Scratch records and credentials are discarded on success and failure.
			await server.stop();
		}
	},
	baseURL: async ({ server }, use) => {
		await use(server.url);
	},
	setupCode: async ({ server }, use) => {
		await use(await server.setupCode());
	}
});
