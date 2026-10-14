// Browser proof of the optional guided setup (#87): people in a batch with a
// printable sign-in sheet, a refused row that keeps its text, and the
// example program as an editable draft. All fixture data is invented.

import { expect, test } from './fixtures';

const PASSWORD = 'invented-passphrase-1';

test('guided setup adds people and starts from the example program', async ({
	page,
	setupCode
}) => {
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

	await page.getByRole('link', { name: 'Guided setup' }).click();
	await expect(page.getByRole('heading', { name: 'Guided setup', level: 1 })).toBeVisible();

	// Three prefilled role rows; names suggest usernames.
	await page.getByLabel('Name, row 1', { exact: true }).fill('Casey Coordinator');
	await expect(page.getByLabel('Username, row 1')).toHaveValue('casey.coordinator');
	await page.getByLabel('Name, row 2', { exact: true }).fill('Jordan Q. Trainer');
	await expect(page.getByLabel('Username, row 2')).toHaveValue('jordan.trainer');
	await page.getByLabel('Name, row 3', { exact: true }).fill('Taylor Trainee');
	// A fourth row reuses the administrator's username and must be refused alone.
	await page.getByRole('button', { name: 'Add another row' }).click();
	await page.getByLabel('Name, row 4', { exact: true }).fill('Duplicate Person');
	await page.getByLabel('Username, row 4').fill('avery.admin');
	await page.getByRole('button', { name: 'Create 4 accounts' }).click();

	const sheet = page.locator('section.sheet');
	await expect(sheet.getByRole('heading', { name: 'Sign-in sheet' })).toBeVisible();
	await expect(sheet.locator('tbody tr')).toHaveCount(3);
	for (const username of ['casey.coordinator', 'jordan.trainer', 'taylor.trainee']) {
		const row = sheet.locator('tbody tr').filter({ hasText: username });
		await expect(row.locator('code.code')).toHaveText(/^[0-9a-f]{32}$/);
	}
	await expect(sheet).toContainText('expire 15 minutes');
	// The refused row stays, with its text and its reason.
	await expect(page.getByRole('alert')).toContainText('avery.admin');
	await expect(page.getByLabel('Username, row 1')).toHaveValue('avery.admin');

	// The example program becomes an editable draft.
	await page.getByRole('button', { name: 'Next: a program' }).click();
	await page.getByRole('button', { name: 'Use the example' }).click();
	await expect(page.getByRole('status')).toContainText('ready as an editable draft');
	await page.getByRole('link', { name: 'Open the draft' }).click();
	await expect(page).toHaveURL(/\/programs\/\d+\/versions\/\d+$/);
	await expect(page.getByLabel('Program name as presented by this version')).toHaveValue(
		'Example Telecommunicator Program'
	);

	// Home's checklist has moved on: people and a program.
	await page.getByRole('navigation', { name: 'Primary' }).getByRole('link', { name: 'Home' }).click();
	await expect(page.locator('section.setup').getByText('2 of 7 steps done')).toBeVisible();

	// Offering the example twice is refused by name, in plain words.
	await page.goto('/welcome');
	await page.getByRole('button', { name: 'Next: a program' }).click();
	await page.getByRole('button', { name: 'Use the example' }).click();
	await expect(page.getByRole('alert')).toContainText('already here');
});
