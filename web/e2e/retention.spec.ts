// Invented policy/hold administration, with explicit authority and no deletion.
import { expect, test } from './fixtures';

test('administer retention authority, policy versions, and hold history', async ({ page, setupCode }) => {
    await page.goto('/');
    await page.getByLabel('Setup code').fill(setupCode);
    await page.getByLabel('Agency name').fill('Invented Retention County');
    await page.getByLabel('Administrator username').fill('avery.admin');
    await page.getByLabel('Administrator display name').fill('Avery Admin');
    await page.getByLabel('Administrator password').fill('invented-passphrase-1');
    await page.getByRole('button', { name: 'Initialize installation' }).click();
    await expect(page).toHaveURL(/\/login$/);
    await page.getByLabel('Username', { exact: true }).fill('avery.admin');
    await page.getByLabel('Password', { exact: true }).fill('invented-passphrase-1');
    await page.getByRole('button', { name: 'Sign in', exact: true }).click();
    await page.getByRole('link', { name: 'Retention', exact: true }).click();
    await expect(page.getByText('Explicit retention administration authority is required to view policies and holds.')).toBeVisible();
    await expect(page.getByRole('heading', { name: 'Policy version', exact: true })).toHaveCount(0);

    await page.getByLabel('User', { exact: true }).selectOption({ label: 'Avery Admin (avery.admin)' });
    await page.getByLabel('Reason for authority change').fill('Invented records custodian appointment');
    await page.getByRole('button', { name: 'Save authority change' }).click();
    await expect(page.getByRole('heading', { name: 'Policy version', exact: true })).toBeVisible();
    await page.getByLabel('Disposition authority reference').fill('INVENTED-SCHEDULE-2026');
    await page.getByLabel('Scheduled action').selectOption('destroy');
    await page.getByLabel('Minimum retention (elapsed days of 24 hours)').fill('365');
    await page.getByLabel('Reason for this version').fill('Invented approved schedule');
    await page.getByRole('button', { name: 'Save policy version' }).click();
    await expect(page.getByRole('heading', { name: 'Daily reports · version 1 (current)' })).toBeVisible();
    await page.getByLabel('Minimum retention (elapsed days of 24 hours)').fill('730');
    await page.getByLabel('Reason for this version').fill('Invented revised schedule');
    await page.getByRole('button', { name: 'Save policy version' }).click();
    await expect(page.getByRole('heading', { name: 'Daily reports · version 2 (current)' })).toBeVisible();
    await expect(page.getByRole('heading', { name: 'Daily reports · version 1 (superseded)' })).toBeVisible();

    await page.getByRole('button', { name: 'New hold', exact: true }).click();
    await page.getByLabel('Hold kind').selectOption('public_records_request');
    await page.getByLabel('Hold authority reference').fill('INVENTED-REQUEST-1');
    await page.getByLabel('Reason for hold').fill('Invented preservation request');
    await page.getByRole('button', { name: 'Place hold', exact: true }).click();
    await expect(page.getByRole('heading', { name: 'Hold 1 · Active · Public records request' })).toBeVisible();
    await page.getByRole('button', { name: 'Replace hold 1', exact: true }).click();
    await page.getByLabel('Hold kind').selectOption('investigation');
    await page.getByLabel('Reason for replacement').fill('Invented updated authority');
    await page.getByRole('button', { name: 'Save replacement hold' }).click();
    await expect(page.getByRole('heading', { name: 'Hold 1 · Released · Public records request' })).toBeVisible();
    await expect(page.getByRole('heading', { name: 'Hold 2 · Active · Investigation' })).toBeVisible();
    await page.getByRole('button', { name: 'Release hold 2', exact: true }).click();
    await page.getByLabel('Reason for release').fill('Invented investigation complete');
    await page.getByRole('button', { name: 'Confirm hold release' }).click();
    await expect(page.getByRole('heading', { name: 'Hold 2 · Released · Investigation' })).toBeVisible();

    await page.getByLabel('Authority change', { exact: true }).selectOption({ label: 'Revoke retention administration' });
    await page.getByLabel('Reason for authority change').fill('Invented reassignment');
    await page.getByRole('button', { name: 'Save authority change' }).click();
    await expect(page.getByRole('heading', { name: 'Policy version', exact: true })).toHaveCount(0);
    const response = await page.request.get('/api/retention/holds');
    expect(response.status()).toBe(403);
    expect((await response.json()).error).toBe('capability_required');
});
