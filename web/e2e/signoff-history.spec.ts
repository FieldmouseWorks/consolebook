// Browser proof for #49: a trainee reads the current state and the complete
// retained history of task signoffs for their own enrollment, including the
// signoffs a program-version change left behind, and gains no write control
// anywhere. All fixture data is invented.

import { expect, test } from './fixtures';
import type { Page } from '@playwright/test';

const PASSWORD = 'invented-passphrase-1';
const TAYLOR_PASSWORD = 'trainee-passphrase-6';
const RILEY_PASSWORD = 'trainee-passphrase-7';

const content = (label: string, prompt: string) => ({
	name: 'Example County CTO Program',
	label,
	description: 'Invented program for signoff-history e2e.',
	phases: [{ name: 'Phase One', description: 'Observation.', presentation_number: 1 }],
	phase_transitions: [],
	competencies: [
		{
			category: 'Call processing',
			name: 'Emergency Call Interrogation',
			description: 'Obtains and verifies location, callback, and nature.',
			tasks: [{ prompt, citations: [] }],
			citations: []
		}
	],
	rating_scales: [
		{
			name: 'Standard 1-7',
			kind: 'anchored_numeric',
			min_value: 1,
			max_value: 7,
			anchors: [{ value: 4, label: 'Meets standards', definition: 'To the invented standard.' }]
		}
	],
	rating_modifiers: [],
	evaluation_forms: [
		{
			record_type: 'daily_report',
			name: 'Daily Observation Report',
			instructions: 'Rate observed performance.',
			competencies: [
				{ competency: 'Emergency Call Interrogation', rating_scale: 'Standard 1-7' }
			],
			narratives: [{ prompt: 'Most acceptable performance.', required: false }]
		}
	],
	citations: [],
	finalization_policy: {
		review_approved: false,
		required_narratives: false,
		ratings_complete: false
	}
});

const FIRST_PROMPT = 'Processes an invented structure-fire call.';
const SECOND_PROMPT = 'Processes an invented medical call.';

async function resetAndLogin(page: Page, username: string, resetCode: string, password: string) {
	await page.goto(`/reset`);
	await expect(page).toHaveURL(/\/reset$/);
	await page.getByLabel('Username').fill(username);
	await page.getByLabel('Reset code').fill(resetCode);
	await page.getByLabel('New password').fill(password);
	const reset = page.waitForResponse((response) => response.url().includes('/api/auth/reset'));
	await page.getByRole('button', { name: 'Set new password' }).click();
	const resetResponse = await reset;
	expect(resetResponse.status(), `reset for ${username}`).toBe(204);
	// The reset redirects to sign-in; the new password is the one to use.
	await expect(page).toHaveURL(/\/login$/);
	await page.getByLabel('Username').fill(username);
	await page.getByLabel('Password').fill(password);
	const signedIn = page.waitForResponse((response) =>
		response.url().includes('/api/auth/login')
	);
	await page.getByRole('button', { name: 'Sign in' }).click();
	const signInResponse = await signedIn;
	expect(signInResponse.status(), `sign in for ${username}`).toBe(200);
	await expect(page).toHaveURL(/\/$/);
}

test('a trainee reads their own complete signoff history', async ({ page, setupCode }) => {
	await page.goto(`/`);
	await expect(page).toHaveURL(/\/setup$/);
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
	await expect(page.getByRole('heading', { name: 'Installation status' })).toBeVisible();

	const program = await (
		await page.request.post(`/api/programs`, { data: { name: content('a', FIRST_PROMPT).name } })
	).json();
	const version = await (
		await page.request.post(`/api/programs/${program.id}/versions`, {
			data: content('2026 rev A', FIRST_PROMPT)
		})
	).json();
	await page.request.post(`/api/program-versions/${version.id}/publish`, { data: {} });
	const trainee = await (
		await page.request.post(`/api/users`, {
			data: { username: 'taylor.trainee', display_name: 'Taylor Trainee' }
		})
	).json();

	const riley = await (
		await page.request.post(`/api/users`, {
			data: { username: 'riley.trainee', display_name: 'Riley Trainee' }
		})
	).json();

	const casey = await (
		await page.request.post(`/api/users`, {
			data: { username: 'casey.coord', display_name: 'Casey Coordinator', role: 'coordinator' }
		})
	).json();
	const enrollment = await (
		await page.request.post(`/api/program-versions/${version.id}/enrollments`, {
			data: { user_id: trainee.id }
		})
	).json();

	// The coordinator signs the task off twice: the first act is a plain
	// signoff, the second an override, which takes review authority and
	// records its reason. The override is the act a version change later
	// leaves behind.
	const matrixResponse = await page.request.get(
		`/api/enrollments/${enrollment.id}/signoffs`
	);
	expect(matrixResponse.status()).toBe(200);
	const matrix = await matrixResponse.json();
	expect(matrix.tasks.length).toBeGreaterThan(0);
	await page.goto('/');
	await page.getByRole('button', { name: 'Sign out' }).click();
	await expect(page).toHaveURL(/\/login$/);
	await resetAndLogin(page, 'casey.coord', casey.reset_code, PASSWORD);
	await page.goto(`/enrollments/${enrollment.id}`);
	await expect(page.getByRole('heading', { name: 'Task signoffs' })).toBeVisible();
	await page.getByRole('button', { name: 'Observed', exact: true }).first().click();
	await expect(page.getByText('by Casey Coordinator', { exact: false }).first()).toBeVisible();
	const overrideReason = 'Re-checked at the invented console.';
	await page.getByPlaceholder('Override reason').first().fill(overrideReason);
	await page.getByRole('button', { name: 'Demonstrated', exact: true }).first().click();
	await expect(page.getByText(overrideReason)).toBeVisible();

	// Back to the administrator for the version change.
	await page.goto('/');
	await page.getByRole('button', { name: 'Sign out' }).click();
	await expect(page).toHaveURL(/\/login$/);
	await page.getByLabel('Username').fill('avery.admin');
	await page.getByLabel('Password').fill(PASSWORD);
	await page.getByRole('button', { name: 'Sign in' }).click();
	await expect(page.getByRole('heading', { name: 'Installation status' })).toBeVisible();

	// A second version, and the enrollment repointed to it: the first
	// version's signoff is now history the current matrix cannot show.
	const nextVersion = await (
		await page.request.post(`/api/programs/${program.id}/versions`, {
			data: content('2026 rev B', SECOND_PROMPT)
		})
	).json();
	await page.request.post(`/api/program-versions/${nextVersion.id}/publish`, { data: {} });
	await page.request.post(`/api/enrollments/${enrollment.id}/events`, {
		data: {
			kind: 'version_change',
			reason: 'Invented move to the new revision.',
			to_version_id: nextVersion.id
		}
	});

	// The trainee signs in and finds their signoffs where their records are.
	await page.goto('/');
	await page.getByRole('button', { name: 'Sign out' }).click();
	await expect(page).toHaveURL(/\/login$/);
	await resetAndLogin(page, 'taylor.trainee', trainee.reset_code, TAYLOR_PASSWORD);
	await page.getByRole('link', { name: 'My records' }).click();
	await expect(page.getByRole('heading', { name: 'My task signoffs' })).toBeVisible();

	// The currently pinned version's task, with no signoff: distinguishable
	// from a revoked one, and stated as such.
	await expect(page.getByText('Not signed off').first()).toBeVisible();
	await expect(page.getByText('1 of 1 task has no signoff.')).toBeVisible();
	await expect(page.getByText(SECOND_PROMPT).first()).toBeVisible();

	// The prior version's signoff stays discoverable and is labelled with
	// the version it was signed under; the full chain is behind one
	// keyboard-reachable disclosure.
	await expect(page.getByRole('heading', { name: 'Earlier program versions' })).toBeVisible();
	const earlier = page.locator('.earlier-tasks li').first();
	await expect(earlier).toContainText(FIRST_PROMPT);
	await expect(earlier).toContainText('2026 rev A (v1)');
	await expect(earlier).toContainText('Demonstrated');
	await expect(earlier).toContainText('Casey Coordinator');
	await expect(earlier).toContainText(overrideReason);
	await earlier.getByText('All 2 recorded signoffs for this task').click();
	await expect(earlier.getByText('Observed', { exact: true })).toBeVisible();
	await expect(earlier).toContainText('Re-checked at the invented console.');

	// Read-only: the trainee gains no recording, override, revoke, or
	// contest control anywhere on the page.
	await expect(page.getByRole('button', { name: 'Observed' })).toHaveCount(0);
	await expect(page.getByRole('button', { name: 'Demonstrated' })).toHaveCount(0);
	await expect(page.getByRole('button', { name: 'Revoke' })).toHaveCount(0);
	await expect(page.getByLabel('Override reason')).toHaveCount(0);

	// The same read is the trainee's own: the API answer matches the page.
	const own = await (
		await page.request.get(`/api/enrollments/${enrollment.id}/signoff-history`)
	).json();
	expect(own.tasks.length).toBeGreaterThan(0);
	expect(own.current_program_version_number).toBe(2);
	expect(own.tasks.find((row: { prompt: string }) => row.prompt === FIRST_PROMPT)).toMatchObject({
		in_current_version: false,
		program_version_number: 1,
		current_kind: 'demonstrated'
	});

	// Another trainee holding the same own-records grant is refused: the
	// read is scoped to the reader's own enrollment, not to trainees.
	await page.goto('/');
	await page.getByRole('button', { name: 'Sign out' }).click();
	await expect(page).toHaveURL(/\/login$/);
	await resetAndLogin(page, 'riley.trainee', riley.reset_code, RILEY_PASSWORD);
	const refused = await page.request.get(`/api/enrollments/${enrollment.id}/signoff-history`);
	expect(refused.status()).toBe(403);
	expect((await refused.json()).error).toBe('capability_required');
});
