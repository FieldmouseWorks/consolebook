// The interface's colors come from light-dark(), which the production CSS
// build lowers to variables that only a declared color-scheme defines.
// Without one, the page background and the header bar silently vanish.

import { expect, test } from './fixtures';

const SCHEMES = [
	{ scheme: 'light', body: 'rgb(244, 245, 247)', header: 'rgb(31, 42, 61)' },
	{ scheme: 'dark', body: 'rgb(22, 24, 29)', header: 'rgb(14, 20, 32)' }
] as const;

for (const { scheme, body, header } of SCHEMES) {
	test(`the shell paints its ${scheme} colors`, async ({ page }) => {
		await page.emulateMedia({ colorScheme: scheme });
		await page.goto('/');
		await expect(page.getByRole('heading', { name: 'First-run setup' })).toBeVisible();
		await expect(page.locator('body')).toHaveCSS('background-color', body);
		await expect(page.locator('header')).toHaveCSS('background-color', header);
	});
}
