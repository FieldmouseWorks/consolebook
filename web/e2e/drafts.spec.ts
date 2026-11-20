// Browser proof of the record lifecycle: collaborative drafts, review,
// finalization, acknowledgment, amendments, summaries, signoffs, exports,
// and trainee packets. Split into focused tests so each stays well inside
// the 60s test budget even under CPU load (#199). All fixture data is invented.

import { expect, signOut, test } from './fixtures';
import type { Browser, BrowserContext, Page } from '@playwright/test';
import { BINARY, execFileAsync } from './server';

const PASSWORD = 'invented-passphrase-1';
const JORDAN_PASSWORD = 'trainer-passphrase-3';
const ROWAN_PASSWORD = 'trainer-passphrase-4';
const CASEY_PASSWORD = 'coordinator-passphrase-5';
const TAYLOR_PASSWORD = 'trainee-passphrase-6';

const content = {
	name: 'Example County CTO Program',
	label: '2026 rev A',
	description: 'Invented program for draft e2e.',
	phases: [
		{ name: 'Phase One', description: 'Observation.', presentation_number: 1 },
		{ name: 'Phase Two', description: 'Guided performance.', presentation_number: 2 }
	],
	phase_transitions: [{ from_phase: 'Phase One', to_phase: 'Phase Two', kind: 'advance' }],
	competencies: [
		{
			category: 'Call processing',
			name: 'Emergency Call Interrogation',
			description: 'Obtains and verifies location, callback, and nature.',
			tasks: [{ prompt: 'Processes an invented structure-fire call.', citations: [] }],
			citations: []
		},
		{
			category: 'Radio',
			name: 'Radio Discipline',
			description: 'Uses clear text and unit identifiers.',
			tasks: [{ prompt: 'Dispatches an invented medical call.', citations: [] }],
			citations: []
		}
	],
	rating_scales: [
		{
			name: 'Standard 1-7',
			kind: 'anchored_numeric',
			min_value: 1,
			max_value: 7,
			anchors: [
				{ value: 1, label: 'Unacceptable', definition: 'Contrary to training.' },
				{ value: 4, label: 'Meets standards', definition: 'To the invented standard.' },
				{ value: 7, label: 'Superior', definition: 'Beyond the invented standard.' }
			]
		},
		{
			name: 'Check',
			kind: 'pass_fail',
			min_value: null,
			max_value: null,
			anchors: [
				{ value: 0, label: 'Fail', definition: 'Did not perform.' },
				{ value: 1, label: 'Pass', definition: 'Performed.' }
			]
		}
	],
	rating_modifiers: [
		{
			code: 'NRT',
			label: 'Not responding to training',
			description: 'Remedial attention documented in the narrative.'
		}
	],
	evaluation_forms: [
		{
			record_type: 'daily_report',
			name: 'Daily Observation Report',
			instructions: 'Rate observed performance.',
			competencies: [
				{ competency: 'Emergency Call Interrogation', rating_scale: 'Standard 1-7' },
				{ competency: 'Radio Discipline', rating_scale: 'Check' }
			],
			narratives: [
				{ prompt: 'Most acceptable performance.', required: true },
				{ prompt: 'Least acceptable performance.', required: false }
			]
		},
		{
			record_type: 'weekly_summary',
			name: 'Weekly Summary',
			instructions: 'Summarize the week.',
			competencies: [],
			narratives: [{ prompt: 'Weekly overview.', required: false }]
		}
	],
	citations: [],
	finalization_policy: {
		review_approved: false,
		required_narratives: true,
		ratings_complete: true
	}
};

async function resetAndLogin(page: Page, username: string, resetCode: string, password: string) {
	await page.goto(`/reset`);
	await page.getByLabel('Username').fill(username);
	await page.getByLabel('Reset code').fill(resetCode);
	await page.getByLabel('New password').fill(password);
	await page.getByRole('button', { name: 'Set new password' }).click();
	await expect(page).toHaveURL(/\/login$/);
	await page.getByLabel('Username').fill(username);
	await page.getByLabel('Password').fill(password);
	await page.getByRole('button', { name: 'Sign in' }).click();
	await expect(page.getByRole('heading', { name: 'Home', level: 1 })).toBeVisible();
}

async function openSignedInContext(
	browser: Browser,
	baseURL: string,
	username: string,
	resetCode: string,
	password: string
): Promise<{ context: BrowserContext; page: Page }> {
	const context = await browser.newContext({ baseURL });
	const page = await context.newPage();
	await resetAndLogin(page, username, resetCode, password);
	return { context, page };
}

interface SeededTraining {
	trainee: { id: number; reset_code: string };
	jordan: { id: number; reset_code: string };
	rowan: { id: number; reset_code: string };
	casey: { id: number; reset_code: string };
	enrollment: { id: number };
	session: { id: number };
}

async function seedTraining(page: Page, setupCode: string): Promise<SeededTraining> {
	// Initialize and sign in as the administrator.
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
	await expect(page.getByRole('heading', { name: 'Home', level: 1 })).toBeVisible();

	// Seed the published program, people, enrollment, assignment, and an
	// open session over the API; the cookie rides page.request.
	const program = await (
		await page.request.post(`/api/programs`, { data: { name: content.name } })
	).json();
	const version = await (
		await page.request.post(`/api/programs/${program.id}/versions`, { data: content })
	).json();
	await page.request.post(`/api/program-versions/${version.id}/publish`, { data: {} });
	const trainee = await (
		await page.request.post(`/api/users`, {
			data: { username: 'taylor.trainee', display_name: 'Taylor Trainee' }
		})
	).json();
	const jordan = await (
		await page.request.post(`/api/users`, {
			data: { username: 'jordan.trainer', display_name: 'Jordan Trainer', role: 'trainer' }
		})
	).json();
	const rowan = await (
		await page.request.post(`/api/users`, {
			data: { username: 'rowan.trainer', display_name: 'Rowan Trainer', role: 'trainer' }
		})
	).json();
	const casey = await (
		await page.request.post(`/api/users`, {
			data: {
				username: 'casey.coord',
				display_name: 'Casey Coordinator',
				role: 'coordinator'
			}
		})
	).json();
	const enrollment = await (
		await page.request.post(`/api/program-versions/${version.id}/enrollments`, {
			data: { user_id: trainee.id }
		})
	).json();
	await page.request.post(`/api/enrollments/${enrollment.id}/assignments`, {
		data: { trainer_user_id: jordan.id }
	});
	const session = await (
		await page.request.post(`/api/enrollments/${enrollment.id}/sessions`, {
			data: {
				business_date: '2026-06-02',
				timezone: 'America/Chicago',
				local_start: '2026-06-02T07:00',
				trainer_user_ids: [jordan.id, rowan.id]
			}
		})
	).json();
	await signOut(page);

	return { trainee, jordan, rowan, casey, enrollment, session };
}

/** Creates, fills, and submits a daily draft over the API. */
async function seedSubmittedDaily(
	ownerPage: Page,
	sessionId: number,
	leastAcceptable: string
): Promise<number> {
	const created = await ownerPage.request.post(`/api/sessions/${sessionId}/draft`, { data: {} });
	expect(created.ok()).toBe(true);
	const { id: draftId } = await created.json();
	const view = await (await ownerPage.request.get(`/api/drafts/${draftId}`)).json();
	const saved = await ownerPage.request.put(`/api/drafts/${draftId}/content`, {
		data: {
			revision: view.revision,
			ratings: [
				{
					form_competency_id: view.form.competencies[0].form_competency_id,
					value: 4,
					not_observed: false,
					modifier_ids: []
				},
				{
					form_competency_id: view.form.competencies[1].form_competency_id,
					value: 1,
					not_observed: false,
					modifier_ids: []
				}
			],
			narratives: [
				{
					form_narrative_id: view.form.narratives[0].form_narrative_id,
					text: 'Ran the invented structure fire cleanly.'
				},
				{
					form_narrative_id: view.form.narratives[1].form_narrative_id,
					text: leastAcceptable
				}
			]
		}
	});
	expect(saved.ok()).toBe(true);
	const { revision } = await saved.json();
	expect(
		(await ownerPage.request.post(`/api/drafts/${draftId}/submit`, { data: { revision } })).ok()
	).toBe(true);
	return draftId;
}

/** Approves and seals version 1 of a submitted daily draft over the API. */
async function seedFinalizedDailyV1(
	rowanPage: Page,
	caseyPage: Page,
	sessionId: number
): Promise<number> {
	const draftId = await seedSubmittedDaily(
		rowanPage,
		sessionId,
		'Callback 555-0100 (invented) now named; corrected.'
	);
	expect(
		(
			await caseyPage.request.post(`/api/drafts/${draftId}/review`, {
				data: { decision: 'approved' }
			})
		).ok()
	).toBe(true);
	const afterReview = await (await caseyPage.request.get(`/api/drafts/${draftId}`)).json();
	expect(
		(
			await caseyPage.request.post(`/api/drafts/${draftId}/finalize`, {
				data: { revision: afterReview.revision }
			})
		).ok()
	).toBe(true);
	return draftId;
}

test('draft, collaborate, transfer, and submit a daily evaluation', async ({
	browser,
	baseURL,
	page,
	setupCode
}) => {
	const seeded = await seedTraining(page, setupCode);
	const contexts: BrowserContext[] = [];
	try {
		// Jordan starts the draft from their own session list.
		await resetAndLogin(page, 'jordan.trainer', seeded.jordan.reset_code, JORDAN_PASSWORD);
		await expect(page.getByRole('heading', { name: 'My sessions' })).toBeVisible();
		await page.getByRole('button', { name: 'Start draft' }).click();
		await expect(page).toHaveURL(new RegExp('/drafts/\\d+$'));
		await expect(
			page.getByRole('heading', { name: 'Daily Observation Report' })
		).toBeVisible();
		await expect(page.getByText('created the draft')).toBeVisible();

		// Rate, mark a modifier, write the narrative; autosave reports Saved
		// and the contribution appears in the attribution stream.
		await page
			.getByLabel('Rate Emergency Call Interrogation')
			.selectOption({ label: '4 — Meets standards' });
		await page.getByLabel('Rate Radio Discipline').selectOption({ label: 'Pass' });
		await page
			.getByLabel('Most acceptable performance.')
			.fill('Ran the invented structure fire cleanly.');
		await expect(page.getByText('Saved', { exact: true })).toBeVisible();
		await page.reload();
		await expect(page.getByLabel('Rate Emergency Call Interrogation')).toHaveValue('4');
		// Anchors are sparse (1, 4, 7); the scale still offers every value.
		await expect(
			page.getByLabel('Rate Emergency Call Interrogation').locator('option[value="2"]')
		).toHaveCount(1);
		// The pinned rubric is one click away: anchor definitions render in
		// the scale guide.
		await page.locator('details.anchors summary').first().click();
		await expect(page.getByText('To the invented standard.')).toBeVisible();
		await expect(page.getByLabel('Most acceptable performance.')).toHaveValue(
			'Ran the invented structure fire cleanly.'
		);
		await expect(page.getByText('Jordan Trainer contributed')).toBeVisible();

		// Hand the draft to Rowan; the submit control leaves with ownership.
		await page.getByLabel('Transfer ownership').selectOption({ label: 'Rowan Trainer' });
		await page.getByRole('button', { name: 'Transfer', exact: true }).click();
		await expect(page.getByText('transferred ownership to')).toBeVisible();
		await expect(page.getByRole('button', { name: 'Submit for review' })).toHaveCount(0);
		const draftUrl = page.url();

		// Rowan finds the session on their list, opens the draft, and submits.
		const rowan = await openSignedInContext(
			browser,
			baseURL!,
			'rowan.trainer',
			seeded.rowan.reset_code,
			ROWAN_PASSWORD
		);
		contexts.push(rowan.context);
		await expect(rowan.page.getByRole('heading', { name: 'My sessions' })).toBeVisible();
		await rowan.page.getByRole('link', { name: 'Open draft' }).click();
		await expect(rowan.page).toHaveURL(draftUrl);
		await rowan.page
			.getByLabel('Least acceptable performance.')
			.fill('Slow unit identifier once; corrected.');
		await expect(rowan.page.getByText('Saved', { exact: true })).toBeVisible();
		await rowan.page.getByRole('button', { name: 'Submit for review' }).click();
		await expect(rowan.page.getByText('Submitted for review', { exact: true })).toBeVisible();
		await expect(rowan.page.getByLabel('Most acceptable performance.')).toBeDisabled();
		await expect(rowan.page.getByLabel('Rate Emergency Call Interrogation')).toBeDisabled();
		await expect(rowan.page.getByRole('button', { name: 'Submit for review' })).toHaveCount(0);

		// The freeze survives a reload; the covered session still shows.
		await rowan.page.reload();
		await expect(rowan.page.getByText('Submitted for review', { exact: true })).toBeVisible();
		await expect(rowan.page.getByText('Session 2026-06-02:')).toBeVisible();
	} finally {
		await Promise.all(contexts.map((context) => context.close()));
	}
});

test('review, request changes, approve, finalize, and export a daily evaluation', async ({
	browser,
	baseURL,
	page,
	setupCode
}) => {
	const seeded = await seedTraining(page, setupCode);
	const contexts: BrowserContext[] = [];
	try {
		const rowan = await openSignedInContext(
			browser,
			baseURL!,
			'rowan.trainer',
			seeded.rowan.reset_code,
			ROWAN_PASSWORD
		);
		contexts.push(rowan.context);
		const draftId = await seedSubmittedDaily(
			rowan.page,
			seeded.session.id,
			'Slow unit identifier once; corrected.'
		);
		const draftUrl = `${baseURL}/drafts/${draftId}`;

		// The coordinator finds the draft on the review queue; a change
		// request needs its comment, then reopens the draft.
		await resetAndLogin(page, 'casey.coord', seeded.casey.reset_code, CASEY_PASSWORD);
		await expect(page.getByRole('heading', { name: 'Review queue' })).toBeVisible();
		await page.getByRole('link', { name: 'Review', exact: true }).click();
		await expect(page).toHaveURL(draftUrl);
		await page.getByLabel('Decision').selectOption({ label: 'Request changes' });
		await expect(page.getByRole('button', { name: 'Decide' })).toBeDisabled();
		await page
			.getByLabel('Comment (required when requesting changes)')
			.fill('Name the invented callback number in the narrative.');
		await page.getByRole('button', { name: 'Decide' }).click();
		await expect(page.getByText('Changes requested', { exact: true })).toBeVisible();
		await expect(
			page.getByText('Change request: Name the invented callback number in the narrative.')
		).toBeVisible();

		// The owner revises the reopened draft and resubmits.
		await rowan.page.goto('/');
		await rowan.page.getByRole('link', { name: 'Open draft' }).click();
		await expect(rowan.page).toHaveURL(draftUrl);
		await rowan.page
			.getByLabel('Least acceptable performance.')
			.fill('Callback 555-0100 (invented) now named; corrected.');
		await expect(rowan.page.getByText('Saved', { exact: true })).toBeVisible();
		await rowan.page.getByRole('button', { name: 'Submit for review' }).click();
		await expect(rowan.page.getByText('Submitted for review', { exact: true })).toBeVisible();

		// The same reviewer approves the resubmission; the draft stays
		// frozen and the verdicts stand in the record.
		await page.goto('/');
		await page.getByRole('link', { name: 'Review', exact: true }).click();
		await page.getByLabel('Decision').selectOption({ label: 'Approve' });
		await page.getByRole('button', { name: 'Decide' }).click();
		await expect(page.getByText('Approved', { exact: true })).toBeVisible();
		await expect(page.getByLabel('Most acceptable performance.')).toBeDisabled();
		await expect(page.getByText('Casey Coordinator requested changes')).toBeVisible();
		await expect(page.getByText('Casey Coordinator approved the draft')).toBeVisible();

		// Finalization seals the approved draft into an immutable version:
		// the sealed view presents from the stored envelope with both
		// fingerprints, and verification reports honest consistency.
		await page.getByRole('button', { name: 'Finalize record' }).click();
		await expect(page.getByText('Finalized', { exact: true })).toBeVisible();
		await expect(page.getByRole('heading', { name: 'Finalized record' })).toBeVisible();
		await expect(page.getByText('finalized by Casey Coordinator')).toBeVisible();
		await expect(
			page.getByText('Ran the invented structure fire cleanly.')
		).toBeVisible();
		await page.getByRole('button', { name: 'Verify hashes' }).click();
		await expect(
			page.getByText('Recomputed from the stored record: both fingerprints match.')
		).toBeVisible();

		// The sealed record leaves as it was stored: the browser download is
		// an archive whose record bytes are the canonical bytes verbatim, and
		// the CLI verifies it from the file alone (ADR 0014).
		const downloadPromise = page.waitForEvent('download');
		await page.getByRole('button', { name: 'Export this version' }).click();
		const download = await downloadPromise;
		expect(download.suggestedFilename()).toMatch(
			/^consolebook-record-\d+-v1-\d{8}T\d{6}Z\.zip$/
		);
		const archivePath = await download.path();
		const verified = await execFileAsync(BINARY, ['export', 'verify', archivePath]);
		expect(verified.stdout).toContain('verified 1 of 1 units');
		await expect(page.getByText(/^Downloaded consolebook-record-/)).toBeVisible();
		// The sealed record takes no further decisions or edits.
		await expect(page.getByRole('button', { name: 'Finalize record' })).toHaveCount(0);
		await expect(page.getByLabel('Most acceptable performance.')).toHaveCount(0);

		// The sealed record awaits the trainee; the reviewer sees the attest
		// controls but the trainee timeline is not theirs.
		await expect(page.getByText('This version awaits acknowledgment.')).toBeVisible();
		await expect(page.getByLabel("Attest on the trainee's behalf")).toBeVisible();
		await expect(page.getByRole('link', { name: 'My records' })).toHaveCount(0);
	} finally {
		await Promise.all(contexts.map((context) => context.close()));
	}
});

test('trainee acknowledges a finalized record, downloads a verified packet, and re-acknowledges an amendment', async ({
	browser,
	baseURL,
	page,
	setupCode
}) => {
	const seeded = await seedTraining(page, setupCode);
	const contexts: BrowserContext[] = [];
	try {
		const rowan = await openSignedInContext(
			browser,
			baseURL!,
			'rowan.trainer',
			seeded.rowan.reset_code,
			ROWAN_PASSWORD
		);
		contexts.push(rowan.context);
		const casey = await openSignedInContext(
			browser,
			baseURL!,
			'casey.coord',
			seeded.casey.reset_code,
			CASEY_PASSWORD
		);
		contexts.push(casey.context);
		const draftId = await seedFinalizedDailyV1(rowan.page, casey.page, seeded.session.id);
		const draftUrl = `${baseURL}/drafts/${draftId}`;

		// Taylor signs in, finds the record on their timeline, and
		// acknowledges receipt with a response.
		await resetAndLogin(page, 'taylor.trainee', seeded.trainee.reset_code, TAYLOR_PASSWORD);
		await page.getByRole('link', { name: 'My records' }).click();
		await expect(page.getByRole('heading', { name: 'My records' })).toBeVisible();
		await expect(page.getByText('Awaiting acknowledgment')).toBeVisible();
		await page.getByRole('link', { name: 'Open' }).click();
		await expect(page).toHaveURL(draftUrl);
		await expect(page.getByRole('heading', { name: 'Finalized record' })).toBeVisible();
		await page
			.getByLabel('Your acknowledgment')
			.selectOption({ label: 'Acknowledge with a response' });
		await page
			.getByLabel('Response')
			.fill('Received; the invented callback correction is noted.');
		await page.getByRole('button', { name: 'Record acknowledgment' }).click();
		await expect(
			page.getByText('Taylor Trainee acknowledged receipt with a response')
		).toBeVisible();
		await expect(
			page.getByText('Received; the invented callback correction is noted.')
		).toBeVisible();
		// One acknowledgment per version: the controls leave with the act.
		await expect(page.getByRole('button', { name: 'Record acknowledgment' })).toHaveCount(0);
		await page.getByRole('link', { name: 'My records' }).click();
		// The trainee leaves with their records: the packet downloads from
		// their own page and verifies from the file alone (ADR 0015).
		await expect(page.getByRole('heading', { name: 'My packets' })).toBeVisible();
		const packetPromise = page.waitForEvent('download');
		await page.getByRole('button', { name: 'Download packet' }).click();
		const packet = await packetPromise;
		expect(packet.suggestedFilename()).toMatch(
			/^consolebook-packet-enrollment-\d+-\d{8}T\d{6}Z\.zip$/
		);
		const packetVerified = await execFileAsync(BINARY, ['export', 'verify', await packet.path()]);
		expect(packetVerified.stdout).toContain('kind          trainee packet');
		expect(packetVerified.stdout).toContain('verified 1 of 1 units and 4 of 4 documents');
		await expect(page.getByText('Acknowledged with response')).toBeVisible();

		// A correction is an amendment, never an edit: the coordinator
		// reopens the sealed record with a reason...
		await casey.page.goto(draftUrl);
		await expect(casey.page.getByRole('heading', { name: 'Amend' })).toBeVisible();
		await casey.page
			.getByLabel('Reason')
			.fill('The invented callback timing was recorded one hour early.');
		await casey.page.getByRole('button', { name: 'Open amendment' }).click();
		await expect(casey.page.getByText('Amendment in progress —')).toBeVisible();
		await expect(casey.page.getByText('Draft', { exact: true })).toBeVisible();

		// ...the owner corrects the reopened copy and resubmits...
		await rowan.page.goto(draftUrl);
		await rowan.page
			.getByLabel('Least acceptable performance.')
			.fill('Amended: callback 555-0100 (invented) at the corrected hour.');
		await expect(rowan.page.getByText('Saved', { exact: true })).toBeVisible();
		await rowan.page.getByRole('button', { name: 'Submit for review' }).click();
		await expect(rowan.page.getByText('Submitted for review', { exact: true })).toBeVisible();

		// ...the reviewer approves and seals version 2, chained to the
		// original, which stays readable in the history with its
		// acknowledgment.
		await casey.page.goto(draftUrl);
		await casey.page.getByLabel('Decision').selectOption({ label: 'Approve' });
		await casey.page.getByRole('button', { name: 'Decide' }).click();
		await expect(casey.page.getByText('Approved', { exact: true })).toBeVisible();
		await casey.page.getByRole('button', { name: 'Finalize record' }).click();
		await expect(casey.page.getByText('Finalized', { exact: true })).toBeVisible();
		await expect(casey.page.getByText('Version 2 · record schema')).toBeVisible();
		await expect(casey.page.getByRole('heading', { name: 'Version history' })).toBeVisible();
		await expect(
			casey.page.getByText('The invented callback timing was recorded one hour early.')
		).toBeVisible();
		await expect(
			casey.page.getByText('Taylor Trainee acknowledged receipt with a response')
		).toBeVisible();
		await casey.page.getByRole('button', { name: 'Verify hashes' }).click();
		await expect(
			casey.page.getByText('Recomputed from the stored record: both fingerprints match.')
		).toBeVisible();

		// Every retained version stays readable: the superseded original
		// opens from the history with its own sealed content.
		await casey.page.getByRole('link', { name: 'Read', exact: true }).click();
		await expect(casey.page.getByText('Superseded version 1')).toBeVisible();
		await expect(
			casey.page.getByText('Callback 555-0100 (invented) now named; corrected.')
		).toBeVisible();
		await casey.page.getByRole('link', { name: 'View the current version' }).click();
		await expect(casey.page.getByText('Version 2 · record schema')).toBeVisible();

		// The successor never inherits acknowledgment: Taylor's timeline
		// shows the amended record awaiting them again.
		await page.goto('/records');
		await expect(page.getByText('Awaiting acknowledgment')).toBeVisible();
		await expect(page.getByText('Amended — v2')).toBeVisible();
		await page.getByRole('link', { name: 'Open' }).click();
		await page.getByRole('button', { name: 'Record acknowledgment' }).click();
		// This version's acknowledgment, not version 1's line in the history.
		const acknowledgment = page.locator('section', {
			has: page.getByRole('heading', { name: 'Acknowledgment', exact: true })
		});
		await expect(acknowledgment.getByText('Taylor Trainee acknowledged receipt')).toBeVisible();
	} finally {
		await Promise.all(contexts.map((context) => context.close()));
	}
});

test('coordinator signs off tasks with override reasons and links a finalized daily version into a weekly summary', async ({
	browser,
	baseURL,
	page,
	setupCode
}) => {
	const seeded = await seedTraining(page, setupCode);
	const contexts: BrowserContext[] = [];
	try {
		const rowan = await openSignedInContext(
			browser,
			baseURL!,
			'rowan.trainer',
			seeded.rowan.reset_code,
			ROWAN_PASSWORD
		);
		contexts.push(rowan.context);
		await resetAndLogin(page, 'casey.coord', seeded.casey.reset_code, CASEY_PASSWORD);

		// Seal v1 then amend to v2 over the API so the weekly summary links v2.
		const draftId = await seedFinalizedDailyV1(rowan.page, page, seeded.session.id);
		expect(
			(
				await page.request.post(`/api/drafts/${draftId}/amend`, {
					data: { reason: 'The invented callback timing was recorded one hour early.' }
				})
			).ok()
		).toBe(true);
		const reopened = await (await rowan.page.request.get(`/api/drafts/${draftId}`)).json();
		expect(
			(
				await rowan.page.request.post(`/api/drafts/${draftId}/submit`, {
					data: { revision: reopened.revision }
				})
			).ok()
		).toBe(true);
		expect(
			(
				await page.request.post(`/api/drafts/${draftId}/review`, {
					data: { decision: 'approved' }
				})
			).ok()
		).toBe(true);
		const afterReview = await (await page.request.get(`/api/drafts/${draftId}`)).json();
		expect(
			(
				await page.request.post(`/api/drafts/${draftId}/finalize`, {
					data: { revision: afterReview.revision }
				})
			).ok()
		).toBe(true);

		// The coordinator signs off tasks on the enrollment page: the first
		// signoff is a plain act, changing it is an override with a reason.
		await page.goto(`/enrollments/${seeded.enrollment.id}`);
		await expect(page.getByRole('heading', { name: 'Task signoffs' })).toBeVisible();
		await page.getByRole('button', { name: 'Observed', exact: true }).first().click();
		await expect(page.getByText('by Casey Coordinator', { exact: false }).first()).toBeVisible();
		await page
			.getByPlaceholder('Override reason')
			.first()
			.fill('Re-checked at the invented console.');
		await page.getByRole('button', { name: 'Demonstrated', exact: true }).first().click();
		await expect(page.getByText('Re-checked at the invented console.')).toBeVisible();

		// A weekly summary links the exact daily version it covers and
		// seals it into the record bytes.
		await page.getByRole('button', { name: 'Start weekly summary' }).click();
		await expect(page).toHaveURL(new RegExp('/drafts/\\d+$'));
		await expect(page.getByRole('heading', { name: 'Covered daily reports' })).toBeVisible();
		await page
			.getByLabel('Link a daily report')
			.selectOption({ label: '2026-06-02 — Daily Observation Report (v2)' });
		await page.getByRole('button', { name: 'Add link' }).click();
		await expect(page.getByText('finalized', { exact: false }).first()).toBeVisible();
		await page
			.getByLabel('Weekly overview.')
			.fill('An invented week of steady progress.');
		await expect(page.getByText('Saved', { exact: true })).toBeVisible();
		await page.getByRole('button', { name: 'Finalize record' }).click();
		await expect(page.getByText('Finalized', { exact: true })).toBeVisible();
		await expect(page.getByRole('heading', { name: 'Covered daily reports' })).toBeVisible();
		await expect(page.getByText('— version 2', { exact: false })).toBeVisible();
		await page.getByRole('button', { name: 'Verify hashes' }).click();
		await expect(
			page.getByText('Recomputed from the stored record: both fingerprints match.')
		).toBeVisible();
	} finally {
		await Promise.all(contexts.map((context) => context.close()));
	}
});
