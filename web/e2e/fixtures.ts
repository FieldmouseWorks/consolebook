import { randomBytes } from 'node:crypto';
import { test as base, expect, type Page } from '@playwright/test';
import { startServer } from './server';

export { expect };

const MIN_PASSWORD_BYTES = 12;
const MAX_PASSWORD_BYTES = 1024;

let cachedPassword: string | undefined;
let cachedWrongPassword: string | undefined;

/**
 * Generates a 41-byte invented password (`invented-` + 32 random hex digits)
 * inside the server's length rules and never equal to a username, mirroring
 * `crates/consolebook-server/tests/support/mod.rs` (#143).
 */
function generateCredential(): string {
	const value = `invented-${randomBytes(16).toString('hex')}`;
	if (value.length < MIN_PASSWORD_BYTES || value.length > MAX_PASSWORD_BYTES) {
		throw new Error('generated test credential outside length bounds');
	}
	return value;
}

/** The default invented password for this worker process. */
export function testPassword(): string {
	cachedPassword ??= generateCredential();
	return cachedPassword;
}

/** A well-formed password that no account in this worker process has. */
export function wrongTestPassword(): string {
	cachedWrongPassword ??= generateCredential();
	if (cachedWrongPassword === testPassword()) {
		throw new Error('wrong test credential collided');
	}
	return cachedWrongPassword;
}

/** A distinct well-formed password on every call, for rotations and other accounts. */
export function freshTestPassword(): string {
	const fresh = generateCredential();
	if (fresh === testPassword()) {
		throw new Error('fresh test credential collided');
	}
	return fresh;
}

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
