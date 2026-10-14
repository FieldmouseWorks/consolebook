// Browser proof of the Milestone 1 shell: setup → sign-in → status →
// issue reset code → reset password → sign back in → sign out.
// All fixture data is invented.

import { expect, test } from './fixtures';

const PASSWORD = 'invented-passphrase-1';
const NEW_PASSWORD = 'rotated-passphrase-2';


test('full shell lifecycle', async ({ page, setupCode }) => {
	// An uninitialized installation routes everything to setup.
	await page.goto('/');
	await expect(page).toHaveURL(/\/setup$/);
	await expect(page.getByRole('heading', { name: 'First-run setup' })).toBeVisible();

	await page.getByLabel('Setup code').fill(setupCode);
	await page.getByLabel('Agency name').fill('Example County Communications');
	await page.getByLabel('Administrator username').fill('avery.admin');
	await page.getByLabel('Administrator display name').fill('Avery Admin');
	await page.getByLabel('Administrator password').fill(PASSWORD);
	await page.getByRole('button', { name: 'Initialize installation' }).click();

	// Setup lands on sign-in; a wrong password is refused generically.
	await expect(page).toHaveURL(/\/login$/);
	await page.getByLabel('Username').fill('avery.admin');
	await page.getByLabel('Password').fill('wrong-password-x');
	await page.getByRole('button', { name: 'Sign in' }).click();
	await expect(page.getByRole('alert')).toContainText('incorrect');

	// Correct password reaches status.
	await page.getByLabel('Password').fill(PASSWORD);
	await page.getByRole('button', { name: 'Sign in' }).click();
	await expect(page.getByRole('heading', { name: 'Home', level: 1 })).toBeVisible();
	await expect(page.getByText('Example County Communications').first()).toBeVisible();
	await page.getByText('About this installation').click();
	await expect(page.getByText('Manage users')).toBeVisible();

	// The notices panel exists and is quiet on a healthy installation.
	await expect(page.getByRole('heading', { name: 'Notices' })).toBeVisible();
	await expect(page.getByText('No notices.')).toBeVisible();

	// The administrator issues themselves a reset code from the UI.
	await page.getByLabel('Username', { exact: true }).fill('avery.admin');
	await page.getByRole('button', { name: 'Issue reset code' }).click();
	const codeOut = page.locator('p.code-out code');
	await expect(codeOut).toBeVisible();
	const resetCode = (await codeOut.textContent())?.trim() ?? '';
	expect(resetCode).toMatch(/^[0-9a-f]{32}$/);

	// Using the code (signed out state) rotates the password and revokes
	// the session that issued it.
	await page.goto('/reset');
	// Still signed in → guard bounces to status; sign out first.
	await expect(page.getByRole('heading', { name: 'Home', level: 1 })).toBeVisible();
	await page.getByRole('button', { name: 'Sign out' }).click();
	await expect(page).toHaveURL(/\/login$/);

	await page.getByRole('link', { name: 'Use a password reset code' }).click();
	await expect(page).toHaveURL(/\/reset$/);
	await page.getByLabel('Username').fill('avery.admin');
	await page.getByLabel('Reset code').fill(resetCode);
	await page.getByLabel('New password').fill(NEW_PASSWORD);
	await page.getByRole('button', { name: 'Set new password' }).click();
	await expect(page).toHaveURL(/\/login$/);

	// Old password dead, new password works.
	await page.getByLabel('Username').fill('avery.admin');
	await page.getByLabel('Password').fill(PASSWORD);
	await page.getByRole('button', { name: 'Sign in' }).click();
	await expect(page.getByRole('alert')).toContainText('incorrect');
	await page.getByLabel('Password').fill(NEW_PASSWORD);
	await page.getByRole('button', { name: 'Sign in' }).click();
	await expect(page.getByRole('heading', { name: 'Home', level: 1 })).toBeVisible();

	// Setup is permanently unavailable: visiting it bounces home.
	await page.goto('/setup');
	await expect(page).toHaveURL(/\/$/);
});

test("the administrator's home walks the first day", async ({ page, setupCode }) => {
	await page.goto('/');
	await page.getByLabel('Setup code').fill(setupCode);
	await page.getByLabel('Agency name').fill('Example County Communications');
	await page.getByLabel('Administrator username').fill('avery.admin');
	await page.getByLabel('Administrator display name').fill('Avery Admin');
	await page.getByLabel('Administrator password').fill(PASSWORD);
	await page.getByRole('button', { name: 'Initialize installation' }).click();
	await expect(page).toHaveURL(/\/login$/);
	await page.getByLabel('Username').fill('avery.admin');
	await page.getByLabel('Password').fill(PASSWORD);
	await page.getByRole('button', { name: 'Sign in' }).click();

	// A fresh installation starts at the beginning of the checklist.
	const checklist = page.locator('section.setup');
	await expect(checklist.getByRole('heading', { name: 'Getting started' })).toBeVisible();
	await expect(checklist.getByText('0 of 8 steps done')).toBeVisible();
	// Without a copy directory, backups are flagged until fixed (ADR 0024).
	const backups = page.locator('#backups');
	await expect(backups.getByRole('alert')).toContainText("only on this server's own disk");
	await expect(backups.locator('pre')).toContainText('--backup-dir');
	await expect(checklist.getByRole('link', { name: /Copy backups off this server/ })).toHaveAttribute(
		'href',
		'#backups'
	);
	await expect(checklist.getByRole('link', { name: /Add people/ })).toHaveAttribute(
		'href',
		'#add-person'
	);

	// The people step needs all three roles; it ticks once the last arrives.
	const people: [string, string][] = [
		['casey.coordinator', 'coordinator'],
		['jordan.trainer', 'trainer']
	];
	for (const [username, role] of people) {
		await page.getByLabel('New username').fill(username);
		await page.getByLabel('Role').selectOption(role);
		await page.getByRole('button', { name: 'Create user' }).click();
		await expect(page.locator('p.code-out')).toContainText(`Created ${username}.`);
	}
	await expect(checklist.getByText('0 of 8 steps done')).toBeVisible();
	await page.getByLabel('New username').fill('taylor.trainee');
	await page.getByLabel('Role').selectOption('trainee');
	await page.getByRole('button', { name: 'Create user' }).click();
	await expect(page.locator('p.code-out')).toContainText('Created taylor.trainee.');
	await expect(checklist.getByText('1 of 8 steps done')).toBeVisible();
	await expect(checklist.locator('li.done')).toContainText('Add people');

	// The checklist is the administrator's; a trainee never sees it.
	const traineeCode = ((await page.locator('p.code-out code').textContent()) ?? '').trim();
	await page.getByRole('button', { name: 'Sign out' }).click();
	await expect(page).toHaveURL(/\/login$/);
	await page.goto('/reset');
	await page.getByLabel('Username').fill('taylor.trainee');
	await page.getByLabel('Reset code').fill(traineeCode);
	await page.getByLabel('New password').fill(NEW_PASSWORD);
	await page.getByRole('button', { name: 'Set new password' }).click();
	await expect(page).toHaveURL(/\/login$/);
	await page.getByLabel('Username').fill('taylor.trainee');
	await page.getByLabel('Password').fill(NEW_PASSWORD);
	await page.getByRole('button', { name: 'Sign in' }).click();
	await expect(page.getByRole('heading', { name: 'Home', level: 1 })).toBeVisible();
	await expect(page.getByRole('heading', { name: 'Getting started' })).toHaveCount(0);
});
