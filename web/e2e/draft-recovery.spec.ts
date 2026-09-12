// Browser proof for #34: the losing writer of a stale save can still read
// and copy the refused text, the winning content stays authoritative, and
// the buffer never reaches the server or survives navigation.
//
// The two writers are real browser contexts against one compiled server and
// one invented draft. Every race is synchronized on a held HTTP request
// rather than a sleep, so the sequence is deterministic.

import { expect, test } from './fixtures';
import type { Browser, BrowserContext, Page } from '@playwright/test';

const PASSWORD = 'invented-passphrase-1';
const JORDAN_PASSWORD = 'trainer-passphrase-3';
const JORDAN = 'jordan.trainer';
const CASEY = 'casey.coord';
const CASEY_PASSWORD = 'coordinator-passphrase-4';
const MOST = 'Most acceptable performance.';
const LEAST = 'Least acceptable performance.';

const content = {
	name: 'Example County CTO Program',
	label: '2026 rev A',
	description: 'Invented program for draft-recovery e2e.',
	phases: [{ name: 'Phase One', description: 'Observation.', presentation_number: 1 }],
	phase_transitions: [],
	competencies: [
		{
			category: 'Call processing',
			name: 'Emergency Call Interrogation',
			description: 'Obtains and verifies location, callback, and nature.',
			tasks: [{ prompt: 'Processes an invented structure-fire call.', citations: [] }],
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
				{ value: 4, label: 'Meets standards', definition: 'To the invented standard.' }
			]
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
			narratives: [
				{ prompt: MOST, required: false },
				{ prompt: LEAST, required: false }
			]
		}
	],
	citations: [],
	finalization_policy: {
		review_approved: false,
		required_narratives: false,
		ratings_complete: false
	}
};

interface Seeded {
	draftUrl: string;
	draftId: number;
	versionId: number;
	jordanUserId: number;
	/** The one-time code the created trainer signs in with. */
	jordanResetCode: string;
	/** The one-time code the created coordinator signs in with. */
	caseyResetCode: string;
}

/** Initialize the installation as the administrator and seed one draft. */
async function seed(
	page: Page,
	browser: Browser,
	setupCode: string
): Promise<Seeded> {
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
			data: { username: JORDAN, display_name: 'Jordan Trainer', role: 'trainer' }
		})
	).json();
	// A coordinator can edit a draft it does not own and holds the review
	// authority that sealing a record takes: the workflow-act recovery test
	// needs a page that can actually attempt the act.
	const casey = await (
		await page.request.post(`/api/users`, {
			data: { username: CASEY, display_name: 'Casey Coordinator', role: 'coordinator' }
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
	const created = await page.request.post(`/api/enrollments/${enrollment.id}/sessions`, {
		data: {
			business_date: '2026-06-02',
				timezone: 'America/Chicago',
			local_start: '2026-06-02T07:00',
			trainer_user_ids: [jordan.id]
		}
	});
	if (!created.ok()) {
		throw new Error(`the seeded session failed: ${created.status()} ${await created.text()}`);
	}
	const session = await created.json();
	// The administrator starts the draft. Jordan is a coordinator, so the
	// losing writer can edit and take a workflow act on a draft it does
	// not own, which is what the recovery scenarios need.
	const draft = await (
		await page.request.post(`/api/sessions/${session.id}/draft`, { data: {} })
	).json();
	return {
		draftUrl: `/drafts/${draft.id}`,
		draftId: draft.id,
		versionId: version.id,
		jordanUserId: jordan.id,
		jordanResetCode: jordan.reset_code,
		caseyResetCode: casey.reset_code
	};
}

/** The second program's form: different prompts, so field identities differ. */
function differentFormContent() {
	return {
		...content,
		name: 'Second Example County Program',
		label: '2026 rev A',
		description: 'Invented second program for the draft-recovery e2e.',
		competencies: [
			{
				category: 'Radio discipline',
				name: 'Emergency Call Prioritization',
				description: 'Orders invented calls by severity.',
				tasks: [{ prompt: 'Prioritizes an invented multi-call incident.', citations: [] }],
				citations: []
			}
		],
		evaluation_forms: [
			{
				record_type: 'daily_report',
				name: 'Daily Observation Report',
				instructions: 'Rate observed performance.',
				competencies: [
					{ competency: 'Emergency Call Prioritization', rating_scale: 'Standard 1-7' }
				],
				narratives: [{ prompt: LEAST, required: false }]
			}
		]
	};
}

/** A second session's draft, which the test starts from the home page. */
interface SecondSession {
	businessDate: string;
	traineeName: string;
}

/**
 * Creates a second trainee's session from a *different* program version, so
 * the draft started there is a different form with different field
 * identities. The draft itself is started in the test, by its owner.
 */
async function secondSession(page: Page, trainerUserId: number): Promise<SecondSession> {
	const program = await (
		await page.request.post(`/api/programs`, { data: { name: 'Second Example County Program' } })
	).json();
	const version = await (
		await page.request.post(`/api/programs/${program.id}/versions`, {
			data: differentFormContent()
		})
	).json();
	await page.request.post(`/api/program-versions/${version.id}/publish`, { data: {} });
	const other = await (
		await page.request.post(`/api/users`, {
			data: { username: 'riley.trainee', display_name: 'Riley Trainee' }
		})
	).json();
	const enrollment = await (
		await page.request.post(`/api/program-versions/${version.id}/enrollments`, {
			data: { user_id: other.id }
		})
	).json();
	const created = await page.request.post(`/api/enrollments/${enrollment.id}/sessions`, {
		data: {
			business_date: '2026-06-03',
			timezone: 'America/Chicago',
			local_start: '2026-06-03T07:00',
			trainer_user_ids: [trainerUserId]
		}
	});
	if (!created.ok()) {
		throw new Error(`the second session failed: ${created.status()} ${await created.text()}`);
	}
	return { businessDate: '2026-06-03', traineeName: 'Riley Trainee' };
}

/** Hands a draft to another author, so the page holds that draft's acts. */
async function transferDraft(page: Page, draftId: number, toUserId: number): Promise<void> {
	const response = await page.request.post(`/api/drafts/${draftId}/transfer`, {
		data: { to_user_id: toUserId }
	});
	if (!response.ok()) {
		throw new Error(`the transfer failed: ${response.status()} ${await response.text()}`);
	}
}

/**
 * Marks the document, so every later hop can prove it was client-side
 * navigation: the lifecycle findings are about component state, which a
 * document reload would discard for reasons of its own.
 */
async function markSpa(page: Page): Promise<void> {
	await page.evaluate(() => {
		(window as unknown as { __spa?: number }).__spa = 1;
	});
}

/** Asserts the document has not been reloaded since {@link markSpa}. */
async function expectSpaAlive(page: Page): Promise<void> {
	expect(
		await page.evaluate(() => (window as unknown as { __spa?: number }).__spa),
		'the document reloaded: this was not client-side navigation'
	).toBe(1);
}

/**
 * Goes straight from one draft to another inside the app. No shipped link
 * offers this transition — every route into a draft passes through another
 * page — so the tests below use the real one; this helper documents the
 * component-reusing transition a guard outside the controller would
 * survive, and is kept for the day such a link exists.
 */
async function spaToDraft(page: Page, draftId: number, traineeName: string): Promise<void> {
	const hop = `spa-hop-${draftId}`;
	await page.evaluate(
		([target, id]) => {
			const link = document.createElement('a');
			link.id = id;
			link.href = target;
			link.textContent = 'invented test navigation';
			link.style.position = 'fixed';
			link.style.left = '0';
			link.style.top = '0';
			link.style.zIndex = '9999';
			document.body.appendChild(link);
		},
		[`/drafts/${draftId}`, hop] as const
	);
	// A trusted click, so the app's router treats it like any other link.
	await page.locator(`#${hop}`).click();
	await expect(page).toHaveURL(new RegExp(`/drafts/${draftId}$`));
	// The destination's own content is what proves the new draft loaded.
	await expect(page.getByText(traineeName)).toBeVisible();
	await expectSpaAlive(page);
}

/** Goes to the home page's session list, inside the app. */
async function homeSessions(page: Page): Promise<void> {
	await page.getByRole('link', { name: 'Home' }).click();
	await expect(page.getByRole('heading', { name: 'My sessions' })).toBeVisible();
	await expectSpaAlive(page);
}

/** Starts a session's draft from the home page, as the author who owns it. */
async function startDraftFromHome(page: Page, businessDate: string): Promise<number> {
	const row = page.locator('tr', { hasText: businessDate });
	await row.getByRole('button', { name: 'Start draft' }).click();
	await expect(page).toHaveURL(/\/drafts\/\d+$/);
	await expect(page.getByRole('heading', { name: 'Daily Observation Report' })).toBeVisible();
	const id = /\/drafts\/(\d+)$/.exec(page.url())?.[1];
	if (id === undefined) {
		throw new Error(`no draft id in ${page.url()}`);
	}
	return Number(id);
}

/** Opens a draft from the home page's session list, inside the app. */
async function openDraftFromHome(
	page: Page,
	businessDate: string,
	draftId: number
): Promise<void> {
	const row = page.locator('tr', { hasText: businessDate });
	await row.getByRole('link', { name: 'Open draft' }).click();
	await expect(page).toHaveURL(new RegExp(`/drafts/${draftId}$`));
	await expect(page.getByRole('heading', { name: 'Daily Observation Report' })).toBeVisible();
}
/** Sign in a newly created trainer through the one-time reset code. */
async function signIn(
	context: BrowserContext,
	username: string,
	resetCode: string,
	password: string
): Promise<Page> {
	const page = await context.newPage();
	await page.goto(`/reset`);
	await page.getByLabel('Username').fill(username);
	await page.getByLabel('Reset code').fill(resetCode);
	await page.getByLabel('New password').fill(password);
	await page.getByRole('button', { name: 'Set new password' }).click();
	await expect(page).toHaveURL(/\/login$/);
	await page.getByLabel('Username').fill(username);
	await page.getByLabel('Password').fill(password);
	await page.getByRole('button', { name: 'Sign in' }).click();
	await expect(page.getByRole('heading', { name: 'Installation status' })).toBeVisible();
	return page;
}

/**
 * A held request: the handler calls `arrive` when it lands and waits on
 * `released`, and the test waits on `arrived` and calls `release`.
 */
function gate(): {
	arrived: Promise<void>;
	released: Promise<void>;
	arrive: () => void;
	release: () => void;
} {
	let arrive: () => void = () => {};
	let release: () => void = () => {};
	const arrived = new Promise<void>((resolve) => {
		arrive = resolve;
	});
	const released = new Promise<void>((resolve) => {
		release = resolve;
	});
	return { arrived, released, arrive, release };
}

/** Waits until the draft is saved and the page reports it. */
async function expectSaved(page: Page): Promise<void> {
	await expect(page.locator('.savestate')).toHaveText('Saved');
}

/**
 * The refused text the panel currently shows for one narrative prompt. The
 * label may carry a note that the text arrived after the refused save was
 * sent, so the match is on the prompt alone.
 */
function refusedText(page: Page, prompt: string) {
	return page
		.locator('details.refused .narrative')
		.filter({ hasText: prompt })
		.locator('pre.refused-text');
}

test('the losing writer recovers their sentence after a stale-save reload', async ({
	page,
	setupCode,
	browser
}) => {
	const seeded = await seed(page, browser, setupCode);
	const loserContext = await browser.newContext();
	const loser = await signIn(loserContext, JORDAN, seeded.jordanResetCode, JORDAN_PASSWORD);
	try {
		// The loser opens the draft and writes their sentence.
		await loser.goto(seeded.draftUrl);
		await expect(loser.getByRole('heading', { name: 'Daily Observation Report' })).toBeVisible();

		// Hold the losing writer's save so the other writer can land first.
		let heldSaves = 0;
		let releaseLoser: () => void = () => {};
		const loserReleased = new Promise<void>((resolve) => {
			releaseLoser = resolve;
		});
		let losingSaveArrived: () => void = () => {};
		const losingSaveHeld = new Promise<void>((resolve) => {
			losingSaveArrived = resolve;
		});
		await loser.route(`**/api/drafts/${seeded.draftId}/content`, async (route) => {
			if (route.request().method() !== 'PUT') {
				await route.continue();
				return;
			}
			heldSaves += 1;
			if (heldSaves === 1) {
				losingSaveArrived();
				await loserReleased;
			}
			await route.continue();
		});

		const losingSentence = 'The invented radio check was lost twice at the north desk.';
		await loser.getByLabel(MOST).fill(losingSentence);
		await losingSaveHeld;

		// The other writer saves first: their copy is the winner.
		await page.goto(seeded.draftUrl);
		const winningSentence = 'Callback 555-0100 (invented) confirmed before dispatch.';
		await page.getByLabel(MOST).fill(winningSentence);
		await expectSaved(page);

		// The held save now reaches the server against a stale revision.
		releaseLoser();
		await expect(loser.getByRole('alert')).toContainText(
			'Another contributor saved first'
		);
		await expect(loser.getByLabel(MOST)).toHaveValue(winningSentence);

		// The refused text is readable and copyable, and it is visibly
		// separated from the winning working copy.
		const panel = loser.locator('details.refused');
		await expect(panel).toBeVisible();
		await expect(panel).toContainText('Your unsaved text from before the reload');
		await expect(refusedText(loser, MOST)).toHaveText(losingSentence);
		// Exactly one PUT was attempted: the refused buffer is never
		// resubmitted on the writer's behalf.
		expect(heldSaves).toBe(1);

		// Nothing about the refusal became record content: the server still
		// holds only the winning sentence.
		const persisted = await (
			await page.request.get(`/api/drafts/${seeded.draftId}`)
		).json();
		const most = persisted.content.narratives.find(
			(entry: { text: string }) => entry.text === losingSentence
		);
		expect(most).toBeUndefined();
		expect(
			persisted.content.narratives.some(
				(entry: { text: string }) => entry.text === winningSentence
			)
		).toBe(true);

		// The writer merges by hand: copy the refused text back in and save
		// through the existing revision contract.
		await loser.getByLabel(MOST).fill(losingSentence);
		await expectSaved(loser);
		const merged = await (
			await page.request.get(`/api/drafts/${seeded.draftId}`)
		).json();
		expect(
			merged.content.narratives.some(
				(entry: { text: string }) => entry.text === losingSentence
			)
		).toBe(true);
		// Their own save is accepted, so the recovery buffer is spent.
		await loser.getByRole('button', { name: 'Discard this text' }).click();
		await expect(panel).toHaveCount(0);
	} finally {
		await loserContext.close();
	}
});

test('text typed while a refused save was pending stays recoverable', async ({
	page,
	setupCode,
	browser
}) => {
	const seeded = await seed(page, browser, setupCode);
	const loserContext = await browser.newContext();
	const loser = await signIn(loserContext, JORDAN, seeded.jordanResetCode, JORDAN_PASSWORD);
	try {
		await loser.goto(seeded.draftUrl);
		await expect(loser.getByRole('heading', { name: 'Daily Observation Report' })).toBeVisible();

		let heldSaves = 0;
		let releaseLoser: () => void = () => {};
		const loserReleased = new Promise<void>((resolve) => {
			releaseLoser = resolve;
		});
		let losingSaveArrived: () => void = () => {};
		const losingSaveHeld = new Promise<void>((resolve) => {
			losingSaveArrived = resolve;
		});
		await loser.route(`**/api/drafts/${seeded.draftId}/content`, async (route) => {
			if (route.request().method() !== 'PUT') {
				await route.continue();
				return;
			}
			heldSaves += 1;
			if (heldSaves === 1) {
				losingSaveArrived();
				await loserReleased;
			}
			await route.continue();
		});

		// The refusal will carry this sentence.
		const refused = 'First draft of the invented handover note.';
		await loser.getByLabel(MOST).fill(refused);
		await losingSaveHeld;

		// While that request is pending, the writer keeps typing: this text
		// was never submitted at all.
		const typedLater = 'Second thought: the invented handover note names the north desk.';
		await loser.getByLabel(LEAST).fill(typedLater);

		// The other writer wins first.
		await page.goto(seeded.draftUrl);
		const winningSentence = 'Callback 555-0100 (invented) confirmed before dispatch.';
		await page.getByLabel(MOST).fill(winningSentence);
		await expectSaved(page);

		const refusedResponse = loser.waitForResponse(
			(response) =>
				response.url().includes(`/api/drafts/${seeded.draftId}/content`) &&
				response.status() === 409
		);
		releaseLoser();
		await refusedResponse;
		await expect(loser.getByRole('alert')).toContainText(
			'Another contributor saved first'
		);

		// The refused sentence is shown under its own prompt...
		await expect(refusedText(loser, MOST)).toHaveText(refused);
		// ...exactly once: a second report of the same refusal would have
		// replaced it with the post-reload state.
		await expect(loser.locator('details.refused')).toHaveCount(1);
		// ...and so is the sentence typed while the request was pending,
		// labelled as never submitted rather than silently dropped.
		await expect(refusedText(loser, LEAST)).toHaveText(typedLater);
		await expect(loser.locator('details.refused .pending-note')).toContainText(
			'never submitted'
		);
		// The winning copy still stands where it was written.
		await expect(loser.getByLabel(MOST)).toHaveValue(winningSentence);
		// One report per refused revision: the retry that carried the
		// never-submitted sentence was refused too, and it did not replace
		// the buffer with the post-reload state. The report count is what
		// proves that, because the buffer itself still holds both pieces
		// of divergent text.
		await expect(loser.locator('details.refused')).toHaveCount(1);
		// Exactly one request reached the server: the refused buffer is
		// never resubmitted on the writer's behalf, and the retry the
		// controller would have made is suppressed while the page has not
		// yet reloaded the winner.
		expect(heldSaves).toBe(1);
		// The retry left the server on the winner's revision, so the
		// refused revision is fully replaced rather than half-applied.
		const after = await (
			await page.request.get(`/api/drafts/${seeded.draftId}`)
		).json();
		expect(
			after.content.narratives.some(
				(entry: { text: string }) => entry.text === typedLater
			)
		).toBe(false);
	} finally {
		await loserContext.close();
	}
});

test('text typed while the winner is being reloaded stays recoverable', async ({
	page,
	setupCode,
	browser
}) => {
	const seeded = await seed(page, browser, setupCode);
	const loserContext = await browser.newContext();
	const loser = await signIn(loserContext, JORDAN, seeded.jordanResetCode, JORDAN_PASSWORD);
	try {
		await loser.goto(seeded.draftUrl);
		await expect(loser.getByRole('heading', { name: 'Daily Observation Report' })).toBeVisible();

		let heldSaves = 0;
		let releaseLoser: () => void = () => {};
		const loserReleased = new Promise<void>((resolve) => {
			releaseLoser = resolve;
		});
		let losingSaveArrived: () => void = () => {};
		const losingSaveHeld = new Promise<void>((resolve) => {
			losingSaveArrived = resolve;
		});
		await loser.route(`**/api/drafts/${seeded.draftId}/content`, async (route) => {
			if (route.request().method() !== 'PUT') {
				await route.continue();
				return;
			}
			heldSaves += 1;
			if (heldSaves === 1) {
				losingSaveArrived();
				await loserReleased;
			}
			await route.continue();
		});

		const refused = 'First draft of the invented handover note.';
		await loser.getByLabel(MOST).fill(refused);
		await losingSaveHeld;

		// The other writer wins first.
		await page.goto(seeded.draftUrl);
		const winningSentence = 'Callback 555-0100 (invented) confirmed before dispatch.';
		await page.getByLabel(MOST).fill(winningSentence);
		await expectSaved(page);

		// The reload the refusal triggers is held open, which is the window
		// a writer keeps typing in: the working copy is still the old one,
		// so the field is live.
		let heldReloads = 0;
		let releaseReload: () => void = () => {};
		const reloadReleased = new Promise<void>((resolve) => {
			releaseReload = resolve;
		});
		let reloadArrived: () => void = () => {};
		const reloadHeld = new Promise<void>((resolve) => {
			reloadArrived = resolve;
		});
		await loser.route(`**/api/drafts/${seeded.draftId}`, async (route) => {
			const path = new URL(route.request().url()).pathname;
			if (route.request().method() !== 'GET' || path !== `/api/drafts/${seeded.draftId}`) {
				await route.continue();
				return;
			}
			heldReloads += 1;
			if (heldReloads === 1) {
				reloadArrived();
				await reloadReleased;
			}
			await route.continue();
		});

		const refusedResponse = loser.waitForResponse(
			(response) =>
				response.url().includes(`/api/drafts/${seeded.draftId}/content`) &&
				response.status() === 409
		);
		releaseLoser();
		await refusedResponse;
		await reloadHeld;

		// Typed while the winner's copy is on its way: this text was in
		// neither the refused request nor the reloaded copy.
		const typedDuringReload = 'Second thought: the invented note names the north desk.';
		await loser.getByLabel(MOST).fill(typedDuringReload);
		releaseReload();

		// The reload replaces the working copy with the winner's...
		await expect(loser.getByLabel(MOST)).toHaveValue(winningSentence);
		// ...and the text typed while it was in flight is still the
		// writer's, labelled as never submitted rather than overwritten
		// unseen.
		await expect(refusedText(loser, MOST)).toHaveText(typedDuringReload);
		await expect(loser.locator('details.refused .pending-note')).toContainText(
			'never submitted'
		);
		// It never reached the server, and the winner's copy is what the
		// draft holds.
		const after = await (
			await page.request.get(`/api/drafts/${seeded.draftId}`)
		).json();
		expect(
			after.content.narratives.some(
				(entry: { text: string }) => entry.text === typedDuringReload
			)
		).toBe(false);
	} finally {
		await loserContext.close();
	}
});

test('a failed reload keeps the refused text and is not reported as a refresh', async ({
	page,
	setupCode,
	browser
}) => {
	const seeded = await seed(page, browser, setupCode);
	const loserContext = await browser.newContext();
	const loser = await signIn(loserContext, JORDAN, seeded.jordanResetCode, JORDAN_PASSWORD);
	try {
		await loser.goto(seeded.draftUrl);
		await expect(loser.getByRole('heading', { name: 'Daily Observation Report' })).toBeVisible();

		let heldSaves = 0;
		let releaseLoser: () => void = () => {};
		const loserReleased = new Promise<void>((resolve) => {
			releaseLoser = resolve;
		});
		let losingSaveArrived: () => void = () => {};
		const losingSaveHeld = new Promise<void>((resolve) => {
			losingSaveArrived = resolve;
		});
		// The reload that follows a refusal fails: the refusal text must
		// survive it, and the page must say the reload failed.
		let refuseReload = false;
		await loser.route(`**/api/drafts/${seeded.draftId}`, async (route) => {
			if (refuseReload && route.request().method() === 'GET') {
				await route.abort('failed');
				return;
			}
			await route.continue();
		});
		await loser.route(`**/api/drafts/${seeded.draftId}/content`, async (route) => {
			if (route.request().method() !== 'PUT') {
				await route.continue();
				return;
			}
			heldSaves += 1;
			if (heldSaves === 1) {
				losingSaveArrived();
				await loserReleased;
			}
			await route.continue();
		});

		const losingSentence = 'The invented console log was misfiled.';
		await loser.getByLabel(MOST).fill(losingSentence);
		await losingSaveHeld;

		await page.goto(seeded.draftUrl);
		const winningSentence = 'Callback 555-0100 (invented) confirmed before dispatch.';
		await page.getByLabel(MOST).fill(winningSentence);
		await expectSaved(page);

		refuseReload = true;
		const refusedResponse = loser.waitForResponse(
			(response) =>
				response.url().includes(`/api/drafts/${seeded.draftId}/content`) &&
				response.status() === 409
		);
		let reloadFailed: () => void = () => {};
		const reloadFailedPromise = new Promise<void>((resolve) => {
			reloadFailed = resolve;
		});
		loser.on('requestfailed', (request) => {
			if (request.url().includes(`/api/drafts/${seeded.draftId}`)) {
				reloadFailed();
			}
		});
		releaseLoser();
		await refusedResponse;
		await reloadFailedPromise;
		await expect(loser.getByRole('alert')).toContainText('reloaded unsuccessfully');
		await expect(loser.locator('details.refused')).toBeVisible();
		await expect(refusedText(loser, MOST)).toHaveText(losingSentence);
	} finally {
		await loserContext.close();
	}
});

test('a second refusal adds to the refused text instead of replacing it', async ({
	page,
	setupCode,
	browser
}) => {
	const seeded = await seed(page, browser, setupCode);
	const loserContext = await browser.newContext();
	const loser = await signIn(loserContext, JORDAN, seeded.jordanResetCode, JORDAN_PASSWORD);
	try {
		await loser.goto(seeded.draftUrl);
		await expect(loser.getByRole('heading', { name: 'Daily Observation Report' })).toBeVisible();

		// Two saves are held in turn, so each can be refused by a save from
		// the other writer that lands first.
		const first = gate();
		const second = gate();
		let savesArrived = 0;
		await loser.route(`**/api/drafts/${seeded.draftId}/content`, async (route) => {
			if (route.request().method() !== 'PUT') {
				await route.continue();
				return;
			}
			savesArrived += 1;
			if (savesArrived === 1) {
				first.arrive();
				await first.released;
			}
			if (savesArrived === 2) {
				second.arrive();
				await second.released;
			}
			await route.continue();
		});

		const firstText = 'First refused sentence of the invented handover.';
		await loser.getByLabel(MOST).fill(firstText);
		await first.arrived;

		// The owner wins the first race.
		await page.goto(seeded.draftUrl);
		const winningOne = 'Callback 555-0100 (invented) confirmed before dispatch.';
		await page.getByLabel(MOST).fill(winningOne);
		await expectSaved(page);
		const firstRefusal = loser.waitForResponse(
			(response) =>
				response.url().includes(`/api/drafts/${seeded.draftId}/content`) &&
				response.status() === 409
		);
		first.release();
		await firstRefusal;
		await expect(refusedText(loser, MOST)).toHaveText(firstText);

		// The writer types again on the reloaded copy, and the owner wins
		// the second race too.
		const secondText = 'Second refused sentence of the invented handover.';
		await loser.getByLabel(LEAST).fill(secondText);
		await second.arrived;
		await page.getByLabel(LEAST).fill('The invented north desk took the callback.');
		await expectSaved(page);
		const secondRefusal = loser.waitForResponse(
			(response) =>
				response.url().includes(`/api/drafts/${seeded.draftId}/content`) &&
				response.status() === 409
		);
		second.release();
		await secondRefusal;

		// Both refusals are kept: the newest first, and the earlier one
		// still there to copy. A later refusal must never bury text the
		// writer has not acknowledged.
		await expect(loser.locator('details.refused')).toHaveCount(2);
		await expect(refusedText(loser, LEAST)).toHaveText(secondText);
		await expect(refusedText(loser, MOST)).toHaveText(firstText);
		await expect(loser.locator('details.refused summary').first()).toContainText(
			'Your unsaved text from before the reload'
		);
		await expect(loser.locator('details.refused summary').nth(1)).toContainText(
			'Text from an earlier refused save'
		);

		// Discarding one leaves the other.
		await loser
			.locator('details.refused')
			.first()
			.getByRole('button', { name: 'Discard this text' })
			.click();
		await expect(loser.locator('details.refused')).toHaveCount(1);
		await expect(refusedText(loser, MOST)).toHaveText(firstText);
	} finally {
		await loserContext.close();
	}
});

test('a workflow act over a refused save is not taken sight unseen', async ({
	page,
	setupCode,
	browser
}) => {
	const seeded = await seed(page, browser, setupCode);

	const loserContext = await browser.newContext();
	const loser = await signIn(loserContext, CASEY, seeded.caseyResetCode, CASEY_PASSWORD);
	try {
		// Casey coordinates this enrollment, so sealing the record is the
		// authority this page really holds; the administrator, who owns the
		// draft, provides the winning save.
		await loser.goto(seeded.draftUrl);
		await expect(loser.getByRole('heading', { name: 'Daily Observation Report' })).toBeVisible();
		await expect(loser.getByRole('button', { name: 'Finalize record' })).toBeVisible();

		let heldSaves = 0;
		let releaseLoser: () => void = () => {};
		const loserReleased = new Promise<void>((resolve) => {
			releaseLoser = resolve;
		});
		let losingSaveArrived: () => void = () => {};
		const losingSaveHeld = new Promise<void>((resolve) => {
			losingSaveArrived = resolve;
		});
		let submits = 0;
		let finalizations = 0;
		await loser.route(`**/api/drafts/${seeded.draftId}/submit`, async (route) => {
			submits += 1;
			await route.continue();
		});
		await loser.route(`**/api/drafts/${seeded.draftId}/finalize`, async (route) => {
			finalizations += 1;
			await route.continue();
		});
		await loser.route(`**/api/drafts/${seeded.draftId}/content`, async (route) => {
			if (route.request().method() !== 'PUT') {
				await route.continue();
				return;
			}
			heldSaves += 1;
			if (heldSaves === 1) {
				losingSaveArrived();
				await loserReleased;
			}
			await route.continue();
		});

		// The coordinator's edit is refused because the owner saves first.
		const losingSentence = 'The invented tone-out was read back twice.';
		await loser.getByLabel(MOST).fill(losingSentence);
		await losingSaveHeld;

		await page.goto(seeded.draftUrl);
		const winningSentence = 'Callback 555-0100 (invented) confirmed before dispatch.';
		await page.getByLabel(MOST).fill(winningSentence);
		await expectSaved(page);

		// The refusal settles first, and the page says so.
		const refused = loser.waitForResponse(
			(response) =>
				response.url().includes(`/api/drafts/${seeded.draftId}/content`) &&
				response.status() === 409
		);
		releaseLoser();
		await refused;
		await expect(loser.getByRole('alert')).toContainText('Another contributor saved first');
		await expect(loser.locator('details.refused')).toBeVisible();

		// The coordinator now asks for both workflow acts over content this
		// page has not seen. Each handler disables its button while it runs,
		// so waiting for the button to come back is what makes "no request
		// was made" a settled fact rather than a race with the click. The
		// refusal stays unresolved across both attempts: one act must not
		// clear the guard for the next.
		const submitButton = loser.getByRole('button', { name: 'Submit for review' });
		await submitButton.click();
		await expect(submitButton).toBeEnabled();
		expect(submits, 'the draft was submitted sight unseen').toBe(0);
		await expect(loser.getByRole('alert')).toContainText('Your refused text is still here');
		const finalizeButton = loser.getByRole('button', { name: 'Finalize record' });
		await finalizeButton.click();
		await expect(finalizeButton).toBeEnabled();
		expect(finalizations, 'the record was sealed sight unseen').toBe(0);
		// It stays editable rather than frozen, and the refused text is
		// still there to copy.
		await expect(loser.getByLabel(MOST)).toHaveValue(winningSentence);
		await expect(loser.locator('details.refused')).toBeVisible();
		await expect(refusedText(loser, MOST)).toHaveText(losingSentence);

		// The writer's acknowledgment is what lifts the guard: discarding
		// the refused text lets the act through.
		await loser.getByRole('button', { name: 'Discard this text' }).click();
		await expect(loser.locator('details.refused')).toHaveCount(0);
		const submitted = loser.waitForResponse((response) =>
			response.url().includes(`/api/drafts/${seeded.draftId}/submit`)
		);
		await submitButton.click();
		const outcome = await submitted;
		expect(outcome.ok(), `submit answered ${outcome.status()}`).toBe(true);
		expect(submits, 'the acknowledged draft was not submitted').toBe(1);
	} finally {
		await loserContext.close();
	}
});

test('a refusal dies with its draft and never blocks another one', async ({
	page,
	setupCode,
	browser
}) => {
	const seeded = await seed(page, browser, setupCode);
	const second = await secondSession(page, seeded.jordanUserId);
	// Jordan owns the first draft, so both drafts in this test have a page
	// whose acts the refusal guard would hold.
	await transferDraft(page, seeded.draftId, seeded.jordanUserId);
	const loserContext = await browser.newContext();
	const loser = await signIn(loserContext, JORDAN, seeded.jordanResetCode, JORDAN_PASSWORD);
	try {
		await loser.goto(seeded.draftUrl);
		// Every hop from here is client-side navigation, and the marker is
		// what proves it.
		await markSpa(loser);
		// The second draft is started first, from its owner's session list,
		// so the refusal below can be followed by a direct draft-to-draft
		// navigation — the one that reuses this route's component.
		await homeSessions(loser);
		const otherId = await startDraftFromHome(loser, second.businessDate);
		await homeSessions(loser);
		await openDraftFromHome(loser, '2026-06-02', seeded.draftId);
		await expect(loser.getByRole('button', { name: 'Submit for review' })).toBeVisible();

		const losingSave = gate();
		await loser.route(`**/api/drafts/${seeded.draftId}/content`, async (route) => {
			if (route.request().method() !== 'PUT') {
				await route.continue();
				return;
			}
			losingSave.arrive();
			await losingSave.released;
			await route.continue();
		});
		let submits = 0;
		await loser.route('**/api/drafts/*/submit', async (route) => {
			submits += 1;
			await route.continue();
		});

		const losingSentence = 'The invented paging test failed on the first attempt.';
		await loser.getByLabel(MOST).fill(losingSentence);
		await losingSave.arrived;

		await page.goto(seeded.draftUrl);
		await page.getByLabel(MOST).fill('Callback 555-0100 (invented) confirmed.');
		await expectSaved(page);

		const refused = loser.waitForResponse(
			(response) =>
				response.url().includes(`/api/drafts/${seeded.draftId}/content`) &&
				response.status() === 409
		);
		losingSave.release();
		await refused;
		await expect(loser.locator('details.refused')).toBeVisible();

		// Another draft, reached the way the app offers: from the home
		// page's session list. It is a different form with its own
		// controller, and it inherits nothing.
		await homeSessions(loser);
		await openDraftFromHome(loser, second.businessDate, otherId);
		await expectSpaAlive(loser);
		await expect(loser.getByLabel(MOST)).toHaveCount(0);
		await expect(loser.locator('details.refused')).toHaveCount(0);
		await expect(loser.getByText(losingSentence)).toHaveCount(0);
		// Not blocked: the act goes through on the new draft.
		const otherSubmitted = loser.waitForResponse((response) =>
			response.url().includes(`/api/drafts/${otherId}/submit`)
		);
		await loser.getByRole('button', { name: 'Submit for review' }).click();
		const otherOutcome = await otherSubmitted;
		expect(otherOutcome.ok(), `submit answered ${otherOutcome.status()}`).toBe(true);
		expect(submits, 'the new draft was not the one submitted').toBe(1);

		// Back to the first draft, whose identity the route matches again:
		// nothing of the refusal comes back with it.
		await homeSessions(loser);
		await openDraftFromHome(loser, '2026-06-02', seeded.draftId);
		await expectSpaAlive(loser);
		await expect(loser.locator('details.refused')).toHaveCount(0);
		await expect(loser.getByText(losingSentence)).toHaveCount(0);
		const ownSubmitted = loser.waitForResponse((response) =>
			response.url().includes(`/api/drafts/${seeded.draftId}/submit`)
		);
		await loser.getByRole('button', { name: 'Submit for review' }).click();
		const ownOutcome = await ownSubmitted;
		expect(ownOutcome.ok(), `submit answered ${ownOutcome.status()}`).toBe(true);
		expect(submits, 'the returned-to draft was not the one submitted').toBe(2);
	} finally {
		await loserContext.close();
	}
});

test('an ordinary edit made just before navigating away is saved', async ({
	page,
	setupCode,
	browser
}) => {
	const seeded = await seed(page, browser, setupCode);
	const loserContext = await browser.newContext();
	const loser = await signIn(loserContext, JORDAN, seeded.jordanResetCode, JORDAN_PASSWORD);
	try {
		await loser.goto(seeded.draftUrl);
		await expect(loser.getByRole('heading', { name: 'Daily Observation Report' })).toBeVisible();

		let saves = 0;
		loser.on('request', (request) => {
			if (request.method() === 'PUT' && request.url().includes('/content')) {
				saves += 1;
			}
		});

		// No refusal here: this is an ordinary debounced autosave.
		const sentence = 'The invented handover note was dictated before the shift change.';
		await loser.getByLabel(MOST).fill(sentence);
		expect(saves, 'the debounce had already fired').toBe(0);

		// Away inside the debounce window: the timer never fired.
		await loser.getByRole('link', { name: 'Home' }).click();
		await expect(loser.getByRole('heading', { name: 'Installation status' })).toBeVisible();

		// The queued edit reaches the server anyway, once.
		await expect
			.poll(
				async () => {
					const view = await (
						await page.request.get(`/api/drafts/${seeded.draftId}`)
					).json();
					return view.content.narratives.some(
						(entry: { text: string }) => entry.text === sentence
					);
				},
				{ message: 'the queued edit never reached the server' }
			)
			.toBe(true);
		expect(saves, 'the edit was saved more than once').toBe(1);
	} finally {
		await loserContext.close();
	}
});

test('copying the refused text back and saving lifts the guard without a discard', async ({
	page,
	setupCode,
	browser
}) => {
	const seeded = await seed(page, browser, setupCode);
	const loserContext = await browser.newContext();
	const loser = await signIn(loserContext, CASEY, seeded.caseyResetCode, CASEY_PASSWORD);
	try {
		await loser.goto(seeded.draftUrl);
		await expect(loser.getByRole('heading', { name: 'Daily Observation Report' })).toBeVisible();

		const losingSave = gate();
		await loser.route(`**/api/drafts/${seeded.draftId}/content`, async (route) => {
			if (route.request().method() !== 'PUT') {
				await route.continue();
				return;
			}
			losingSave.arrive();
			await losingSave.released;
			await route.continue();
		});

		const losingSentence = 'The invented tone-out was read back twice.';
		await loser.getByLabel(MOST).fill(losingSentence);
		await losingSave.arrived;
		await page.goto(seeded.draftUrl);
		await page.getByLabel(MOST).fill('Callback 555-0100 (invented) confirmed before dispatch.');
		await expectSaved(page);
		const refused = loser.waitForResponse(
			(response) =>
				response.url().includes(`/api/drafts/${seeded.draftId}/content`) &&
				response.status() === 409
		);
		losingSave.release();
		await refused;
		await expect(refusedText(loser, MOST)).toHaveText(losingSentence);

		// The recovery instructions, followed exactly: copy the text back
		// into the reloaded field and let the autosave carry it.
		await loser.getByLabel(MOST).fill(losingSentence);
		await expectSaved(loser);

		// The act goes through without the writer also discarding the panel,
		// and the panel is still there for them.
		await expect(loser.locator('details.refused')).toBeVisible();
		const finalized = loser.waitForResponse((response) =>
			response.url().includes(`/api/drafts/${seeded.draftId}/finalize`)
		);
		await loser.getByRole('button', { name: 'Finalize record' }).click();
		const outcome = await finalized;
		expect(outcome.ok(), `finalize answered ${outcome.status()}`).toBe(true);
	} finally {
		await loserContext.close();
	}
});

test('saving one refusal back does not resolve another refusal\'s text', async ({
	page,
	setupCode,
	browser
}) => {
	const seeded = await seed(page, browser, setupCode);
	const loserContext = await browser.newContext();
	const loser = await signIn(loserContext, CASEY, seeded.caseyResetCode, CASEY_PASSWORD);
	try {
		await loser.goto(seeded.draftUrl);
		await expect(loser.getByRole('heading', { name: 'Daily Observation Report' })).toBeVisible();

		// Two refused saves, each beaten by a save from the owner.
		const first = gate();
		const second = gate();
		let savesArrived = 0;
		await loser.route(`**/api/drafts/${seeded.draftId}/content`, async (route) => {
			if (route.request().method() !== 'PUT') {
				await route.continue();
				return;
			}
			savesArrived += 1;
			if (savesArrived === 1) {
				first.arrive();
				await first.released;
			}
			if (savesArrived === 2) {
				second.arrive();
				await second.released;
			}
			await route.continue();
		});
		let finalizations = 0;
		await loser.route(`**/api/drafts/${seeded.draftId}/finalize`, async (route) => {
			finalizations += 1;
			await route.continue();
		});

		const firstText = 'The invented radio check was missed at shift change.';
		await loser.getByLabel(MOST).fill(firstText);
		await first.arrived;
		await page.goto(seeded.draftUrl);
		await page.getByLabel(MOST).fill('Callback 555-0100 (invented) confirmed before dispatch.');
		await expectSaved(page);
		const firstRefusal = loser.waitForResponse(
			(response) =>
				response.url().includes(`/api/drafts/${seeded.draftId}/content`) &&
				response.status() === 409
		);
		first.release();
		await firstRefusal;
		await expect(refusedText(loser, MOST)).toHaveText(firstText);

		const secondText = 'The invented log entry named the wrong north desk.';
		await loser.getByLabel(LEAST).fill(secondText);
		await second.arrived;
		await page.getByLabel(LEAST).fill('The invented north desk took the callback.');
		await expectSaved(page);
		const secondRefusal = loser.waitForResponse(
			(response) =>
				response.url().includes(`/api/drafts/${seeded.draftId}/content`) &&
				response.status() === 409
		);
		second.release();
		await secondRefusal;
		await expect(loser.locator('details.refused')).toHaveCount(2);

		// Putting back the newest refusal's text is a real save, and it
		// resolves that refusal only: the earlier text is still not in the
		// draft, so the act still waits.
		await loser.getByLabel(LEAST).fill(secondText);
		await expectSaved(loser);
		const finalizeButton = loser.getByRole('button', { name: 'Finalize record' });
		await finalizeButton.click();
		await expect(finalizeButton).toBeEnabled();
		expect(finalizations, 'an act dropped the earlier refused text').toBe(0);
		await expect(loser.getByRole('alert')).toContainText('Your refused text is still here');

		// Putting the earlier text back too resolves it, and the act goes
		// through without any discard.
		await loser.getByLabel(MOST).fill(firstText);
		await expectSaved(loser);
		const finalized = loser.waitForResponse((response) =>
			response.url().includes(`/api/drafts/${seeded.draftId}/finalize`)
		);
		await finalizeButton.click();
		const outcome = await finalized;
		expect(outcome.ok(), `finalize answered ${outcome.status()}`).toBe(true);
		expect(finalizations).toBe(1);
	} finally {
		await loserContext.close();
	}
});

test('a refusal whose reload lands after the page moved on changes nothing', async ({
	page,
	setupCode,
	browser
}) => {
	const seeded = await seed(page, browser, setupCode);
	const second = await secondSession(page, seeded.jordanUserId);
	await transferDraft(page, seeded.draftId, seeded.jordanUserId);
	const loserContext = await browser.newContext();
	const loser = await signIn(loserContext, JORDAN, seeded.jordanResetCode, JORDAN_PASSWORD);
	try {
		await loser.goto(seeded.draftUrl);
		await markSpa(loser);
		await homeSessions(loser);
		const otherId = await startDraftFromHome(loser, second.businessDate);
		await homeSessions(loser);
		await openDraftFromHome(loser, '2026-06-02', seeded.draftId);

		const losingSave = gate();
		await loser.route(`**/api/drafts/${seeded.draftId}/content`, async (route) => {
			if (route.request().method() !== 'PUT') {
				await route.continue();
				return;
			}
			losingSave.arrive();
			await losingSave.released;
			await route.continue();
		});
		// The reload a refusal triggers is held open, so the page can leave
		// before its answer arrives.
		const reload = gate();
		await loser.route(`**/api/drafts/${seeded.draftId}`, async (route) => {
			const path = new URL(route.request().url()).pathname;
			if (route.request().method() !== 'GET' || path !== `/api/drafts/${seeded.draftId}`) {
				await route.continue();
				return;
			}
			reload.arrive();
			await reload.released;
			await route.continue();
		});

		const losingSentence = 'The invented handover note was never signed.';
		await loser.getByLabel(MOST).fill(losingSentence);
		await losingSave.arrived;
		await page.goto(seeded.draftUrl);
		await page.getByLabel(MOST).fill('Callback 555-0100 (invented) confirmed.');
		await expectSaved(page);
		const refused = loser.waitForResponse(
			(response) =>
				response.url().includes(`/api/drafts/${seeded.draftId}/content`) &&
				response.status() === 409
		);
		losingSave.release();
		await refused;
		await reload.arrived;

		// Leave for the other draft, inside the app, and then let the
		// obsolete answer land.
		await homeSessions(loser);
		await openDraftFromHome(loser, second.businessDate, otherId);
		reload.release();

		// The destination is untouched: no buffer, no refused text, no error
		// painted onto it, and its act is not held.
		await expect(loser.locator('details.refused')).toHaveCount(0);
		await expect(loser.getByText(losingSentence)).toHaveCount(0);
		await expect(loser.getByRole('alert')).toHaveCount(0);
		const otherSubmitted = loser.waitForResponse((response) =>
			response.url().includes(`/api/drafts/${otherId}/submit`)
		);
		await loser.getByRole('button', { name: 'Submit for review' }).click();
		const otherOutcome = await otherSubmitted;
		expect(otherOutcome.ok(), `submit answered ${otherOutcome.status()}`).toBe(true);

		// And back on the first draft, nothing stale appears either.
		await homeSessions(loser);
		await openDraftFromHome(loser, '2026-06-02', seeded.draftId);
		await expectSpaAlive(loser);
		await expect(loser.locator('details.refused')).toHaveCount(0);
		await expect(loser.getByText(losingSentence)).toHaveCount(0);
		await expect(loser.getByRole('alert')).toHaveCount(0);
	} finally {
		await loserContext.close();
	}
});
