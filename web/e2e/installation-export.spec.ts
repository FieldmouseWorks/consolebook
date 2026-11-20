// Browser proof of the complete installation export (#98; ADR 0028;
// docs/formats/installation-export.md): an export_records holder downloads
// it from the home page before any record is finalized, the CLI verifies it
// from the file alone, and a person without export_records neither sees the
// action nor gets the archive. All fixture data is invented.

import http from 'node:http';
import type { AddressInfo } from 'node:net';

import { expect, freshTestPassword, signOut, test, testPassword } from './fixtures';
import { BINARY, execFileAsync } from './server';

const ADMIN_PASSWORD = testPassword();
const TRAINER_PASSWORD = freshTestPassword();

test('an administrator exports the whole installation; others are refused', async ({
	page,
	setupCode
}) => {
	await page.goto('/');
	await expect(page).toHaveURL(/\/setup$/);
	await page.getByLabel('Setup code').fill(setupCode);
	await page.getByLabel('Agency name').fill('Example County Communications');
	await page.getByLabel('Administrator username').fill('avery.admin');
	await page.getByLabel('Administrator display name').fill('Avery Admin');
	await page.getByLabel('Administrator password').fill(ADMIN_PASSWORD);
	await page.getByRole('button', { name: 'Initialize installation' }).click();
	await expect(page).toHaveURL(/\/login$/);
	await page.getByLabel('Username').fill('avery.admin');
	await page.getByLabel('Password').fill(ADMIN_PASSWORD);
	await page.getByRole('button', { name: 'Sign in' }).click();
	await expect(page.getByRole('heading', { name: 'Home', level: 1 })).toBeVisible();

	// A person but no finalized record: the records-only export has nothing
	// to carry, while the whole installation still has someone to leave with.
	const trainer = await (
		await page.request.post('/api/users', {
			data: { username: 'jordan.trainer', display_name: 'Jordan Trainer', role: 'trainer' }
		})
	).json();
	await page.reload();
	await expect(
		page.getByText('No finalized records yet; there is no record export to make.')
	).toBeVisible();
	await expect(page.getByRole('button', { name: 'Export every finalized record' })).toHaveCount(
		0
	);

	const downloadPromise = page.waitForEvent('download');
	await page.getByRole('button', { name: 'Export the whole installation' }).click();
	const download = await downloadPromise;
	expect(download.suggestedFilename()).toMatch(
		/^consolebook-installation-export-\d{8}T\d{6}Z\.zip$/
	);
	// execFile rejects on a non-zero exit, so resolving is the exit-0 proof.
	const verified = await execFileAsync(BINARY, ['export', 'verify', await download.path()]);
	expect(verified.stdout).toContain('kind          installation export');
	expect(verified.stdout).toMatch(
		/^verified 0 of 0 units and (\d+) of \1 documents: the installation export is consistent with its stated fingerprints$/m
	);
	await expect(page.getByRole('status').filter({ hasText: 'Downloaded' })).toHaveText(
		`Downloaded ${download.suggestedFilename()}.`
	);
	await signOut(page);

	// Negative control: a trainer holds no export_records, so the home page
	// offers no export and the route refuses their session outright.
	await page.goto('/reset');
	await page.getByLabel('Username').fill('jordan.trainer');
	await page.getByLabel('Reset code').fill(trainer.reset_code);
	await page.getByLabel('New password').fill(TRAINER_PASSWORD);
	await page.getByRole('button', { name: 'Set new password' }).click();
	await expect(page).toHaveURL(/\/login$/);
	await page.getByLabel('Username').fill('jordan.trainer');
	await page.getByLabel('Password').fill(TRAINER_PASSWORD);
	await page.getByRole('button', { name: 'Sign in' }).click();
	await expect(page.getByRole('heading', { name: 'Home', level: 1 })).toBeVisible();
	await expect(page.getByRole('button', { name: 'Export the whole installation' })).toHaveCount(
		0
	);
	const refused = await page.request.get('/api/exports/installation');
	expect(refused.status()).toBe(403);
	expect((await refused.json()).error).toBe('capability_required');
});

type ExportFault = 'pass' | 'truncate' | 'abort' | 'refuse';

/**
 * Transparent loopback proxy in front of the real server that can inject
 * transport-level export failures on `/api/exports/installation` without
 * mocking browser `fetch`.
 */
async function startExportFaultProxy(upstreamUrl: string) {
	const upstream = new URL(upstreamUrl);
	let mode: ExportFault = 'pass';
	const server = http.createServer((req, res) => {
		if (req.method === 'GET' && req.url === '/api/exports/installation') {
			if (mode === 'abort') {
				req.socket.destroy();
				return;
			}
			if (mode === 'refuse') {
				res.writeHead(503, { 'Content-Type': 'application/json' });
				res.end(
					JSON.stringify({
						error: 'export_busy',
						message: 'too many exports are already running; try again in a moment'
					})
				);
				return;
			}
			if (mode === 'truncate') {
				// Send valid 200 OK chunked headers and one partial chunk, then close
				// cleanly (TCP FIN) without the terminating `0\r\n\r\n` chunk so the
				// browser receives the response headers and fails while reading the body.
				req.socket.write(
					'HTTP/1.1 200 OK\r\n' +
						'Content-Type: application/zip\r\n' +
						'Content-Disposition: attachment; filename="truncated-export.zip"\r\n' +
						'Transfer-Encoding: chunked\r\n' +
						'Connection: close\r\n' +
						'\r\n' +
						'10\r\n' +
						'PK\x03\x04truncated000\r\n',
					() => req.socket.end()
				);
				return;
			}
		}

		const forward = http.request(
			{
				hostname: upstream.hostname,
				port: Number(upstream.port),
				path: req.url,
				method: req.method,
				headers: { ...req.headers, host: upstream.host }
			},
			(upstreamRes) => {
				res.writeHead(upstreamRes.statusCode ?? 502, upstreamRes.headers);
				upstreamRes.pipe(res);
			}
		);
		forward.on('error', () => {
			if (!res.headersSent) {
				res.writeHead(502);
			}
			res.end();
		});
		req.pipe(forward);
	});

	await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
	const { port } = server.address() as AddressInfo;
	return {
		url: `http://127.0.0.1:${port}`,
		setMode(next: ExportFault) {
			mode = next;
		},
		close() {
			server.closeAllConnections();
			return new Promise<void>((resolve) => server.close(() => resolve()));
		}
	};
}

test('an export cut off mid-transfer reports an incomplete transfer without saving a partial archive', async ({
	page,
	server,
	setupCode
}) => {
	const proxy = await startExportFaultProxy(server.url);
	let downloads = 0;
	page.on('download', () => {
		downloads += 1;
	});

	try {
		await page.goto(`${proxy.url}/`);
		await expect(page).toHaveURL(/\/setup$/);
		await page.getByLabel('Setup code').fill(setupCode);
		await page.getByLabel('Agency name').fill('Example County Communications');
		await page.getByLabel('Administrator username').fill('avery.admin');
		await page.getByLabel('Administrator display name').fill('Avery Admin');
		await page.getByLabel('Administrator password').fill(ADMIN_PASSWORD);
		await page.getByRole('button', { name: 'Initialize installation' }).click();
		await expect(page).toHaveURL(/\/login$/);
		await page.getByLabel('Username').fill('avery.admin');
		await page.getByLabel('Password').fill(ADMIN_PASSWORD);
		await page.getByRole('button', { name: 'Sign in' }).click();
		await expect(page.getByRole('heading', { name: 'Home', level: 1 })).toBeVisible();

		const exportButton = page.getByRole('button', { name: 'Export the whole installation' });
		const alert = page.locator('section.card', { hasText: 'Installation exports' }).getByRole('alert');

		// 1. Transfer starts (200 OK) then ends without its terminal chunk.
		proxy.setMode('truncate');
		await exportButton.click();
		await expect(alert).toHaveText('the export did not finish; download it again');
		expect(downloads).toBe(0);

		// 2. Typed server refusal preserves the server's own message.
		proxy.setMode('refuse');
		await exportButton.click();
		await expect(alert).toHaveText(
			'too many exports are already running; try again in a moment'
		);
		expect(downloads).toBe(0);

		// 3. Connection failure before headers keeps the unreachable message.
		proxy.setMode('abort');
		await exportButton.click();
		await expect(alert).toHaveText('the server could not be reached');
		expect(downloads).toBe(0);

		// 4. Positive control: unobstructed retry downloads and verifies cleanly.
		proxy.setMode('pass');
		const downloadPromise = page.waitForEvent('download');
		await exportButton.click();
		const download = await downloadPromise;
		expect(downloads).toBe(1);
		const verified = await execFileAsync(BINARY, ['export', 'verify', await download.path()]);
		expect(verified.stdout).toContain('kind          installation export');
	} finally {
		await proxy.close();
	}
});
