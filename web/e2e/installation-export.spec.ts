// Browser proof of the complete installation export (#98; ADR 0028;
// docs/formats/installation-export.md): an export_records holder downloads
// it from the home page before any record is finalized, the CLI verifies it
// from the file alone, and a person without export_records neither sees the
// action nor gets the archive. All fixture data is invented.

import { expect, signOut, test } from './fixtures';
import { BINARY, execFileAsync } from './server';

const ADMIN_PASSWORD = 'invented-passphrase-1';
const TRAINER_PASSWORD = 'trainer-passphrase-2';

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
