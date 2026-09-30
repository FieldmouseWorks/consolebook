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
	enrollmentId: number;
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
	setupCode: string,
	withSummary = false
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
	const versionContent = withSummary
		? {
			...content,
			evaluation_forms: [
				...content.evaluation_forms,
				{
					record_type: 'weekly_summary',
					name: 'Weekly Summary',
					instructions: 'Summarize invented training.',
					competencies: [],
					narratives: [
						{ prompt: 'Weekly overview.', required: false },
						{ prompt: 'Weekly follow-up.', required: false }
					]
				}
			]
		}
		: content;
	const version = await (
		await page.request.post(`/api/programs/${program.id}/versions`, { data: versionContent })
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
	// The administrator starts the draft. Jordan can contribute as its
	// assigned trainer; Casey can review and finalize as coordinator.
	const draft = await (
		await page.request.post(`/api/sessions/${session.id}/draft`, { data: {} })
	).json();
	return {
		draftUrl: `/drafts/${draft.id}`,
		draftId: draft.id,
		enrollmentId: enrollment.id,
		versionId: version.id,
		jordanUserId: jordan.id,
		jordanResetCode: jordan.reset_code,
		caseyResetCode: casey.reset_code
	};
}

/** Finalize a seed daily, then start a weekly draft with that daily linkable. */
async function seedWeekly(page: Page, browser: Browser, setupCode: string) {
	const seeded = await seed(page, browser, setupCode, true);
	const daily = await (await page.request.get(`/api/drafts/${seeded.draftId}`)).json();
	const coordinatorContext = await browser.newContext();
	try {
		const coordinator = await signIn(
			coordinatorContext, CASEY, seeded.caseyResetCode, CASEY_PASSWORD
		);
		const finalized = await coordinator.request.post(`/api/drafts/${seeded.draftId}/finalize`, {
			data: { revision: daily.revision }
		});
		if (!finalized.ok()) {
			throw new Error(`seed daily did not finalize: ${finalized.status()} ${await finalized.text()}`);
		}
		const created = await page.request.post(
			`/api/enrollments/${seeded.enrollmentId}/weekly-summary`,
			{ data: {} }
		);
		if (!created.ok()) {
			throw new Error(`seed weekly draft failed: ${created.status()} ${await created.text()}`);
		}
		const summaryId: number = (await created.json()).id;
		const linkable = await (
			await page.request.get(`/api/drafts/${summaryId}/linkable-dailies`)
		).json();
		const dailyVersionId: number | undefined = linkable.dailies[0]?.daily_version_id;
		if (dailyVersionId === undefined) throw new Error('the finalized seed daily is not linkable');
		return {
			...seeded,
			coordinatorContext,
			coordinator,
			summaryId,
			summaryUrl: `/drafts/${summaryId}`,
			dailyVersionId
		};
	} catch (error) {
		await coordinatorContext.close();
		throw error;
	}
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
	sessionId: number;
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
	return { sessionId: (await created.json()).id, businessDate: '2026-06-03', traineeName: 'Riley Trainee' };
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

/** Navigates through a real SvelteKit link, including when the page has no app controls. */
async function spaToPath(page: Page, target: string): Promise<void> {
	const expectedUrl = new URL(target, page.url()).href;
	await page.evaluate((href) => {
		document.getElementById('test-draft-route-hop')?.remove();
		const link = document.createElement('a');
		link.id = 'test-draft-route-hop';
		link.href = href;
		link.textContent = 'invented test navigation';
		link.style.position = 'fixed';
		link.style.left = '0';
		link.style.top = '0';
		link.style.zIndex = '9999';
		document.body.appendChild(link);
	}, target);
	await page.locator('#test-draft-route-hop').click();
	await expect(page).toHaveURL(expectedUrl);
	await expectSpaAlive(page);
}

/** Rejects a route before any draft API request and renders its route error. */
async function expectInvalidDraftRoute(page: Page, draftGets: string[]): Promise<void> {
	await expect.poll(async () => ({
		alerts: (await page.getByRole('alert').allTextContents()).map((text) => text.trim()),
		loading: await page.getByText('Loading…', { exact: true }).count() > 0,
		draftGets
	}), { message: 'the invalid route should settle without loading a draft' }).toEqual({
		alerts: ['Invalid draft or version URL.'],
		loading: false,
		draftGets: []
	});
}

/** Returns to a valid route through SPA navigation and proves editing still saves. */
async function returnToEditableDraft(page: Page, draftId: number): Promise<void> {
	await spaToPath(page, `/drafts/${draftId}`);
	await expect(page.getByRole('heading', { name: 'Daily Observation Report' })).toBeVisible();
	await expect(page.getByRole('alert')).toHaveCount(0);
	const narrative = page.getByLabel(MOST);
	await expect(narrative).toBeEnabled();
	const savedText = 'Invented text after an invalid route was corrected.';
	const saved = page.waitForResponse((response) =>
		response.request().method() === 'PUT' &&
		new URL(response.url()).pathname === `/api/drafts/${draftId}/content`
	);
	await narrative.fill(savedText);
	const response = await saved;
	expect(response.ok()).toBe(true);
	await expectSaved(page);
	const persisted = await (await page.request.get(`/api/drafts/${draftId}`)).json();
	expect(
		persisted.content.narratives.some((entry: { text: string }) => entry.text === savedText)
	).toBe(true);
}

/**
 * Goes straight from one draft to another inside the app. No shipped link
 * offers this transition — every route into a draft passes through another
 * page. A trusted injected link exercises that component-reusing route
 * transition, including its controller teardown and next draft load.
 */
async function spaToDraft(page: Page, draftId: number, traineeName: string): Promise<void> {
	const hop = `spa-hop-${draftId}`;
	const requests: string[] = [];
	const errors: string[] = [];
	const onRequest = (request: { url: () => string }) => {
		if (request.url().includes('/api/drafts/')) requests.push(request.url());
	};
	const onPageError = (error: Error) => errors.push(error.message);
	page.on('request', onRequest);
	page.on('pageerror', onPageError);
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
	try {
		await expect(page.getByText(traineeName)).toBeVisible();
	} catch (error) {
		const marker = await page.evaluate(() => (window as unknown as { __spa?: number }).__spa);
		throw new Error(
			`direct draft navigation did not render ${traineeName}; requests=${JSON.stringify(requests)}; page errors=${JSON.stringify(errors)}; marker=${marker}`,
			{ cause: error }
		);
	} finally {
		page.off('request', onRequest);
		page.off('pageerror', onPageError);
	}
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

/** Hold the first successful PUT response after the real server has applied it. */
async function holdAppliedSave(
	page: Page,
	draftId: number,
	second: ReturnType<typeof gate> | null = null,
	abortSecond = false
) {
	const first = gate();
	const revisions: number[] = [];
	await page.route(`**/api/drafts/${draftId}/content`, async (route) => {
		if (route.request().method() !== 'PUT') {
			await route.continue();
			return;
		}
		revisions.push(route.request().postDataJSON().revision);
		if (revisions.length === 1) {
			const response = await route.fetch();
			if (!response.ok()) {
				throw new Error(`the first save failed: ${response.status()}`);
			}
			first.arrive();
			await first.released;
			await route.fulfill({ response });
			return;
		}
		if (revisions.length === 2 && second !== null) {
			second.arrive();
			await second.released;
			if (abortSecond) {
				await route.abort('failed');
				return;
			}
		}
		await route.continue();
	});
	return { first, revisions };
}

/** Hold the final metadata refresh after an ordinary save reaches the server. */
async function holdDraftMetadata(page: Page, draftId: number) {
	const metadata = gate();
	await page.route(`**/api/drafts/${draftId}`, async (route) => {
		if (route.request().method() !== 'GET' ||
			new URL(route.request().url()).pathname !== `/api/drafts/${draftId}`) {
			await route.continue();
			return;
		}
		const response = await route.fetch();
		if (!response.ok()) throw new Error(`metadata refresh failed: ${response.status()}`);
		metadata.arrive();
		await metadata.released;
		await route.fulfill({ response });
	});
	return metadata;
}

/** Attempt a real tab close and reject the native unsaved-changes prompt. */
async function dismissTabClose(page: Page): Promise<void> {
	const prompted = page.waitForEvent('dialog', { timeout: 5_000 });
	await page.close({ runBeforeUnload: true });
	const dialog = await prompted;
	expect(dialog.type()).toBe('beforeunload');
	await dialog.dismiss();
	expect(page.isClosed(), 'dismissing the warning should keep the draft open').toBe(false);
}

/** Waits until the draft is saved and the page reports it. */
async function expectSaved(page: Page): Promise<void> {
	await expect(page.locator('.savestate')).toHaveText('Saved');
}

/** Wait until no controller still cancels a document unload. */
async function expectUnloadReady(page: Page): Promise<void> {
	await expect.poll(async () => page.evaluate(() => {
		const event = new Event('beforeunload', { cancelable: true });
		window.dispatchEvent(event);
		return event.defaultPrevented;
	}), { message: 'the ordinary save chain still holds the unload guard' }).toBe(false);
}

/** A saved label appears before the metadata GET releases the save chain. */
async function settleDraftMetadata(
	page: Page,
	draftId: number,
	metadata: ReturnType<typeof gate>
): Promise<void> {
	const answered = page.waitForResponse((response) =>
		response.request().method() === 'GET' &&
		new URL(response.url()).pathname === `/api/drafts/${draftId}`
	);
	metadata.release();
	await answered;
	await page.waitForLoadState('networkidle');
	await expectUnloadReady(page);
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

test('an invalid draft path reports its URL and recovers through SPA navigation', async ({
	page,
	setupCode,
	browser
}) => {
	const seeded = await seed(page, browser, setupCode);
	await page.goto(seeded.draftUrl);
	await expect(page.getByRole('heading', { name: 'Daily Observation Report' })).toBeVisible();
	await markSpa(page);
	const draftGets: string[] = [];
	const onRequest = (request: { method: () => string; url: () => string }) => {
		if (request.method() === 'GET' && new URL(request.url()).pathname.startsWith('/api/drafts/')) {
			draftGets.push(new URL(request.url()).pathname);
		}
	};
	page.on('request', onRequest);
	try {
		await spaToPath(page, '/drafts/foo');
		await expectInvalidDraftRoute(page, draftGets);
	} finally {
		page.off('request', onRequest);
	}
	await returnToEditableDraft(page, seeded.draftId);
});

test('an invalid version query reports its URL and recovers through SPA navigation', async ({
	page,
	setupCode,
	browser
}) => {
	const seeded = await seed(page, browser, setupCode);
	await page.goto(seeded.draftUrl);
	await expect(page.getByRole('heading', { name: 'Daily Observation Report' })).toBeVisible();
	await markSpa(page);
	const draftGets: string[] = [];
	const onRequest = (request: { method: () => string; url: () => string }) => {
		if (request.method() === 'GET' && new URL(request.url()).pathname.startsWith('/api/drafts/')) {
			draftGets.push(new URL(request.url()).pathname);
		}
	};
	page.on('request', onRequest);
	try {
		await spaToPath(page, `/drafts/${seeded.draftId}?version=abc`);
		await expectInvalidDraftRoute(page, draftGets);
	} finally {
		page.off('request', onRequest);
	}
	await returnToEditableDraft(page, seeded.draftId);
});

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
		await expect(panel).toContainText('Your local text from before the reload');
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
			'Your local text from before the reload'
		);
		await expect(loser.locator('details.refused summary').nth(1)).toContainText(
			'Earlier local text'
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

test('an ordinary edit saves before a retried SPA departure', async ({
	page,
	setupCode,
	browser
}) => {
	const seeded = await seed(page, browser, setupCode);
	const loserContext = await browser.newContext();
	const loser = await signIn(loserContext, JORDAN, seeded.jordanResetCode, JORDAN_PASSWORD);
	const saveReply = gate();
	try {
		await loser.goto(seeded.draftUrl);
		await expect(loser.getByRole('heading', { name: 'Daily Observation Report' })).toBeVisible();

		let saves = 0;
		loser.on('request', (request) => {
			if (request.method() === 'PUT' && request.url().includes('/content')) {
				saves += 1;
			}
		});
		await loser.route(`**/api/drafts/${seeded.draftId}/content`, async (route) => {
			if (route.request().method() !== 'PUT') {
				await route.continue();
				return;
			}
			const response = await route.fetch();
			if (!response.ok()) throw new Error(`mounted save failed: ${response.status()}`);
			saveReply.arrive();
			await saveReply.released;
			await route.fulfill({ response });
		});

		// No refusal here: this is an ordinary debounced autosave.
		const sentence = 'The invented handover note was dictated before the shift change.';
		await loser.getByLabel(MOST).fill(sentence);
		expect(saves, 'the debounce had already fired').toBe(0);

		// Leaving inside the debounce window starts a mounted save. The
		// writer leaves deliberately after seeing that it finished.
		await loser.getByRole('link', { name: 'Home' }).click();
		await saveReply.arrived;
		await expect(loser).toHaveURL(new RegExp(`${seeded.draftUrl}$`));
		await expect(loser.getByRole('alert')).toContainText('Try leaving again after it saves');
		saveReply.release();
		await expectSaved(loser);
		await expectUnloadReady(loser);
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
		saveReply.release();
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

test('a retry reload answered after leaving cannot affect another draft', async ({
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
		// The automatic winner load fails, leaving a visible buffer. A
		// deliberate retry may be abandoned while its GET is still in flight.
		const reload = gate();
		let reloads = 0;
		await loser.route(`**/api/drafts/${seeded.draftId}`, async (route) => {
			const path = new URL(route.request().url()).pathname;
			if (route.request().method() !== 'GET' || path !== `/api/drafts/${seeded.draftId}`) {
				await route.continue();
				return;
			}
			reloads += 1;
			if (reloads === 1) {
				await route.abort('failed');
				return;
			}
			const response = await route.fetch();
			reload.arrive();
			await reload.released;
			await route.fulfill({ response });
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
		await expect(refusedText(loser, MOST)).toHaveText(losingSentence);
		await expect(loser.getByRole('button', { name: 'Reload latest draft' })).toBeVisible();
		await loser.getByRole('button', { name: 'Reload latest draft' }).click();
		await reload.arrived;

		// The refusal was surfaced before departure. The retry answer still
		// belongs to the old controller and cannot paint the destination.
		await homeSessions(loser);
		await openDraftFromHome(loser, second.businessDate, otherId);
		const oldAnswer = loser.waitForResponse((response) =>
			response.url().endsWith(`/api/drafts/${seeded.draftId}`) && response.status() === 200
		);
		reload.release();
		await oldAnswer;
		await loser.waitForLoadState('networkidle');

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

test('a retried navigation saves the later edit with the accepted revision', async ({
	page,
	setupCode,
	browser
}) => {
	const seeded = await seed(page, browser, setupCode);
	const second = await secondSession(page, seeded.jordanUserId);
	const authorContext = await browser.newContext();
	const author = await signIn(authorContext, JORDAN, seeded.jordanResetCode, JORDAN_PASSWORD);
	try {
		await markSpa(author);
		const otherId = await startDraftFromHome(author, second.businessDate);
		await homeSessions(author);
		await openDraftFromHome(author, '2026-06-02', seeded.draftId);
		const held = await holdAppliedSave(author, seeded.draftId);
		await author.getByLabel(MOST).fill('First invented note from the north desk.');
		await held.first.arrived;
		const later = 'Second invented note from the north desk.';
		await author.getByLabel(MOST).fill(later);
		await author.getByRole('link', { name: 'Home' }).click();
		await expect(author).toHaveURL(new RegExp(`${seeded.draftUrl}$`));
		await expect(author.getByLabel(MOST)).toHaveValue(later);
		await expect(author.getByRole('alert')).toContainText('Try leaving again after it saves');
		held.first.release();

		await expect.poll(async () => {
			const draft = await (await page.request.get(`/api/drafts/${seeded.draftId}`)).json();
			return draft.content.narratives.some((entry: { text: string }) => entry.text === later);
		}, { message: 'the later edit was lost after navigation' }).toBe(true);
		expect(held.revisions).toHaveLength(2);
		expect(held.revisions[1]).toBe(held.revisions[0] + 1);
		await expectUnloadReady(author);
		await spaToDraft(author, otherId, second.traineeName);
		const other = await (await page.request.get(`/api/drafts/${otherId}`)).json();
		expect(other.content.narratives).toEqual([]);
		await expectSpaAlive(author);
	} finally {
		await authorContext.close();
	}
});

for (const resolution of ['copy back', 'discard'] as const) {
	test(`a refused narrative deletion stays visible until ${resolution}`, async ({
		page,
		setupCode,
		browser
	}) => {
		const seeded = await seed(page, browser, setupCode);
		const original = 'Invented baseline note kept on the winning draft.';
		await page.goto(seeded.draftUrl);
		await page.getByLabel(MOST).fill(original);
		await expectSaved(page);
		const authorContext = await browser.newContext();
		const author = await signIn(authorContext, CASEY, seeded.caseyResetCode, CASEY_PASSWORD);
		const losing = gate();
		try {
			await author.goto(seeded.draftUrl);
			await author.route(`**/api/drafts/${seeded.draftId}/content`, async (route) => {
				if (route.request().method() !== 'PUT') {
					await route.continue();
					return;
				}
				losing.arrive();
				await losing.released;
				await route.continue();
			});
			let finalizations = 0;
			await author.route(`**/api/drafts/${seeded.draftId}/finalize`, async (route) => {
				finalizations += 1;
				await route.continue();
			});
			await author.getByLabel(MOST).fill('');
			await losing.arrived;
			await page.getByLabel(LEAST).fill('The invented callback was checked.');
			await expectSaved(page);
			// Return the winner's other field to its original value. The only
			// divergent field is then the writer's intended deletion.
			await page.getByLabel(LEAST).fill('');
			await expectSaved(page);
			losing.release();
			await expect(author.getByLabel(MOST)).toHaveValue(original);
			const deletion = author.locator('details.refused .narrative').filter({ hasText: MOST });
			await expect(deletion).toContainText('Clear this narrative');
			await expect(author.locator('details.refused')).toHaveCount(1);
			const finalize = author.getByRole('button', { name: 'Finalize record' });
			await finalize.click();
			await expect(author.getByRole('alert')).toContainText('Your refused text is still here');
			expect(finalizations).toBe(0);
			if (resolution === 'copy back') {
				await author.getByLabel(MOST).fill('');
				await expectSaved(author);
				await expect(author.locator('details.refused')).toHaveCount(1);
			} else {
				await author.getByRole('button', { name: 'Discard this text' }).click();
				await expect(author.locator('details.refused')).toHaveCount(0);
			}
			const submitted = author.waitForResponse((response) =>
				response.url().includes(`/api/drafts/${seeded.draftId}/finalize`)
			);
			await finalize.click();
			expect((await submitted).ok()).toBe(true);
			expect(finalizations).toBe(1);
			const final = await (await page.request.get(`/api/drafts/${seeded.draftId}`)).json();
			expect(final.content.narratives.some((entry: { text: string }) => entry.text === original))
				.toBe(resolution === 'discard');
		} finally {
			losing.release();
			await authorContext.close();
		}
	});
}

test('only a successfully submitted snapshot resolves refused text', async ({
	page,
	setupCode,
	browser
}) => {
	const seeded = await seed(page, browser, setupCode);
	const authorContext = await browser.newContext();
	const author = await signIn(authorContext, CASEY, seeded.caseyResetCode, CASEY_PASSWORD);
	const losing = gate();
	const applied = gate();
	const meta = gate();
	try {
		await author.goto(seeded.draftUrl);
		await author.route(`**/api/drafts/${seeded.draftId}/content`, async (route) => {
			if (route.request().method() !== 'PUT') {
				await route.continue();
				return;
			}
			losing.arrive();
			await losing.released;
			await route.continue();
		});
		const refused = 'Invented radio check from the east desk.';
		const winner = 'Invented callback from the west desk.';
		await author.getByLabel(MOST).fill(refused);
		await losing.arrived;
		await page.goto(seeded.draftUrl);
		await page.getByLabel(MOST).fill(winner);
		await expectSaved(page);
		losing.release();
		await expect(refusedText(author, MOST)).toHaveText(refused);

		let saves = 0;
		await author.unroute(`**/api/drafts/${seeded.draftId}/content`);
		await author.route(`**/api/drafts/${seeded.draftId}`, async (route) => {
			if (route.request().method() !== 'GET' ||
				new URL(route.request().url()).pathname !== `/api/drafts/${seeded.draftId}`) {
				await route.continue();
				return;
			}
			meta.arrive();
			await meta.released;
			await route.continue();
		});
		await author.route(`**/api/drafts/${seeded.draftId}/content`, async (route) => {
			if (route.request().method() !== 'PUT') {
				await route.continue();
				return;
			}
			saves += 1;
			if (saves === 1) {
				const response = await route.fetch();
				applied.arrive();
				await applied.released;
				await route.fulfill({ response });
				return;
			}
			await route.continue();
		});
		let finalizations = 0;
		await author.route(`**/api/drafts/${seeded.draftId}/finalize`, async (route) => {
			finalizations += 1;
			await route.continue();
		});
		// This request carries the winner's text, plus an unrelated edit.
		await author.getByLabel(LEAST).fill('Invented unrelated note.');
		await applied.arrived;
		// The refused text exists only in the live field while that older
		// request is settling. It was absent from the submitted snapshot.
		await author.getByLabel(MOST).fill(refused);
		applied.release();
		await meta.arrived;
		await author.getByLabel(MOST).fill(winner);
		meta.release();
		await expect.poll(() => saves, { message: 'the later edit was not saved' }).toBeGreaterThan(1);
		await expectSaved(author);
		const finalize = author.getByRole('button', { name: 'Finalize record' });
		await finalize.click();
		await expect(author.getByRole('alert')).toContainText('Your refused text is still here');
		expect(finalizations, 'a live-only match incorrectly cleared the recovery guard').toBe(0);
	} finally {
		losing.release();
		applied.release();
		meta.release();
		await authorContext.close();
	}
});

test('a pending refusal reload holds workflow acts and keeps later typing', async ({
	page,
	setupCode,
	browser
}) => {
	const seeded = await seed(page, browser, setupCode);
	const authorContext = await browser.newContext();
	const author = await signIn(authorContext, CASEY, seeded.caseyResetCode, CASEY_PASSWORD);
	const losing = gate();
	const reload = gate();
	try {
		await author.goto(seeded.draftUrl);
		await expect(author.getByRole('heading', { name: 'Daily Observation Report' })).toBeVisible();
		await author.route(`**/api/drafts/${seeded.draftId}/content`, async (route) => {
			if (route.request().method() !== 'PUT') {
				await route.continue();
				return;
			}
			losing.arrive();
			await losing.released;
			await route.continue();
		});
		let reloads = 0;
		await author.route(`**/api/drafts/${seeded.draftId}`, async (route) => {
			if (route.request().method() !== 'GET' ||
				new URL(route.request().url()).pathname !== `/api/drafts/${seeded.draftId}`) {
				await route.continue();
				return;
			}
			reloads += 1;
			if (reloads === 1) {
				reload.arrive();
				await reload.released;
			}
			await route.continue();
		});
		let finalizations = 0;
		await author.route(`**/api/drafts/${seeded.draftId}/finalize`, async (route) => {
			finalizations += 1;
			await route.continue();
		});
		const refused = 'Invented dispatch note from the first desk.';
		const later = 'Invented dispatch note typed during the reload.';
		await author.getByLabel(MOST).fill(refused);
		await losing.arrived;
		await page.goto(seeded.draftUrl);
		await page.getByLabel(MOST).fill('Invented winning callback note.');
		await expectSaved(page);
		losing.release();
		await reload.arrived;
		await author.getByLabel(LEAST).fill(later);
		const finalize = author.getByRole('button', { name: 'Finalize record' });
		await finalize.click();
		await expect(author.getByRole('alert')).toContainText('Your refused text is still here');
		expect(finalizations).toBe(0);
		expect(reloads).toBe(1);
		reload.release();
		await expect(refusedText(author, MOST)).toHaveText(refused);
		await expect(refusedText(author, LEAST)).toHaveText(later);
	} finally {
		losing.release();
		reload.release();
		await authorContext.close();
	}
});

test('an old reload cannot adopt into a new visit to the same draft', async ({
	page,
	setupCode,
	browser
}) => {
	const seeded = await seed(page, browser, setupCode);
	const second = await secondSession(page, seeded.jordanUserId);
	const authorContext = await browser.newContext();
	const author = await signIn(authorContext, JORDAN, seeded.jordanResetCode, JORDAN_PASSWORD);
	const losing = gate();
	const oldReload = gate();
	try {
		await markSpa(author);
		const otherId = await startDraftFromHome(author, second.businessDate);
		await homeSessions(author);
		await openDraftFromHome(author, '2026-06-02', seeded.draftId);
		await author.route(`**/api/drafts/${seeded.draftId}/content`, async (route) => {
			if (route.request().method() !== 'PUT') {
				await route.continue();
				return;
			}
			losing.arrive();
			await losing.released;
			await route.continue();
		});
		let reloads = 0;
		await author.route(`**/api/drafts/${seeded.draftId}`, async (route) => {
			if (route.request().method() !== 'GET' ||
				new URL(route.request().url()).pathname !== `/api/drafts/${seeded.draftId}`) {
				await route.continue();
				return;
			}
			reloads += 1;
			if (reloads === 1) {
				await route.abort('failed');
				return;
			}
			if (reloads === 2) {
				const response = await route.fetch();
				oldReload.arrive();
				await oldReload.released;
				await route.fulfill({ response });
				return;
			}
			await route.continue();
		});
		await author.getByLabel(MOST).fill('Invented refused note from the first visit.');
		await losing.arrived;
		await page.goto(seeded.draftUrl);
		await page.getByLabel(MOST).fill('Invented winning note from the east desk.');
		await expectSaved(page);
		losing.release();
		await expect(author.getByRole('button', { name: 'Reload latest draft' })).toBeVisible();
		await author.getByRole('button', { name: 'Reload latest draft' }).click();
		await oldReload.arrived;
		await spaToDraft(author, otherId, second.traineeName);
		await spaToDraft(author, seeded.draftId, 'Taylor Trainee');
		const fresh = 'Invented fresh note from the second visit.';
		await author.getByLabel(MOST).fill(fresh);
		await expectSaved(author);
		const oldAnswer = author.waitForResponse((response) =>
			response.url().endsWith(`/api/drafts/${seeded.draftId}`) && response.status() === 200
		);
		oldReload.release();
		await oldAnswer;
		await author.waitForLoadState('networkidle');
		await expect(author.getByLabel(MOST)).toHaveValue(fresh);
		await expect(author.locator('details.refused')).toHaveCount(0);
		await expect(author.getByRole('alert')).toHaveCount(0);
		await expectSpaAlive(author);
	} finally {
		losing.release();
		oldReload.release();
		await authorContext.close();
	}
});

test('a workflow act waits for an edit made during its in-flight save', async ({
	page,
	setupCode,
	browser
}) => {
	const seeded = await seed(page, browser, setupCode);
	const authorContext = await browser.newContext();
	const author = await signIn(authorContext, CASEY, seeded.caseyResetCode, CASEY_PASSWORD);
	try {
		await author.goto(seeded.draftUrl);
		const held = await holdAppliedSave(author, seeded.draftId);
		await author.getByLabel(MOST).fill('Invented first desk note.');
		await held.first.arrived;
		const finalize = author.getByRole('button', { name: 'Finalize record' });
		await finalize.click();
		await expect(finalize).toBeDisabled();
		await author.clock.pauseAt(new Date());
		const later = 'Invented second desk note typed while finalization waited.';
		await author.getByLabel(LEAST).fill(later);
		held.first.release();
		await expect(author.getByText('Finalized', { exact: true })).toBeVisible();
		const finalized = await (await page.request.get(`/api/drafts/${seeded.draftId}`)).json();
		expect(finalized.status).toBe('finalized');
		expect(finalized.content.narratives.some((entry: { text: string }) => entry.text === later))
			.toBe(true);
		expect(held.revisions).toHaveLength(2);
		expect(held.revisions[1]).toBe(held.revisions[0] + 1);
	} finally {
		await authorContext.close();
	}
});

test('a workflow response from one draft cannot act on the next draft', async ({
	page,
	setupCode,
	browser
}) => {
	const seeded = await seed(page, browser, setupCode);
	const second = await secondSession(page, seeded.jordanUserId);
	await transferDraft(page, seeded.draftId, seeded.jordanUserId);
	const authorContext = await browser.newContext();
	const author = await signIn(authorContext, JORDAN, seeded.jordanResetCode, JORDAN_PASSWORD);
	const submitted = gate();
	try {
		await markSpa(author);
		const otherId = await startDraftFromHome(author, second.businessDate);
		await homeSessions(author);
		await openDraftFromHome(author, '2026-06-02', seeded.draftId);
		await author.route(`**/api/drafts/${seeded.draftId}/submit`, async (route) => {
			const response = await route.fetch();
			if (!response.ok()) throw new Error(`source submit failed: ${response.status()}`);
			submitted.arrive();
			await submitted.released;
			await route.fulfill({ response });
		});
		let otherSubmits = 0;
		await author.route(`**/api/drafts/${otherId}/submit`, async (route) => {
			otherSubmits += 1;
			await route.continue();
		});
		await author.getByLabel(MOST).fill('Invented source draft note.');
		await expectSaved(author);
		await expectUnloadReady(author);
		const submit = author.getByRole('button', { name: 'Submit for review' });
		await submit.click();
		await expect(submit).toBeDisabled();
		await submitted.arrived;
		await spaToDraft(author, otherId, second.traineeName);
		submitted.release();
		await author.waitForLoadState('networkidle');
		const other = await (await page.request.get(`/api/drafts/${otherId}`)).json();
		expect(other.status).toBe('draft');
		expect(otherSubmits, 'the old act reached the destination draft').toBe(0);
		await expectSpaAlive(author);
	} finally {
		submitted.release();
		await authorContext.close();
	}
});

for (const act of [
	{ label: 'Submit for review', path: 'submit', status: 'submitted' },
	{ label: 'Finalize record', path: 'finalize', status: 'finalized' }
] as const) {
	test(`editing pauses while the ${act.path} request is in flight`, async ({
	page,
	setupCode,
	browser
}) => {
	const seeded = await seed(page, browser, setupCode);
	const authorContext = act.path === 'finalize' ? await browser.newContext() : null;
	const author = authorContext === null
		? page
		: await signIn(authorContext, CASEY, seeded.caseyResetCode, CASEY_PASSWORD);
	const held = gate();
	try {
		await author.goto(seeded.draftUrl);
		await expect(author.getByLabel(MOST)).toBeVisible();
		await author.getByLabel(MOST).fill('Invented note saved before the workflow act.');
		await expectSaved(author);
		await author.route(`**/api/drafts/${seeded.draftId}/${act.path}`, async (route) => {
			const response = await route.fetch();
			if (!response.ok()) throw new Error(`${act.path} failed: ${response.status()}`);
			held.arrive();
			await held.released;
			await route.fulfill({ response });
		});
		await author.getByRole('button', { name: act.label }).click();
		await held.arrived;
		await expect(author.getByLabel(MOST)).toBeDisabled();
		await expect(author.getByLabel(LEAST)).toBeDisabled();
		held.release();
		await expect(author.getByText(
			act.status === 'submitted' ? 'Submitted for review' : 'Finalized',
			{ exact: true }
		)).toBeVisible();
		await expect.poll(async () => {
			const draft = await (await page.request.get(`/api/drafts/${seeded.draftId}`)).json();
			return draft.status;
		}).toBe(act.status);
	} finally {
		held.release();
		await authorContext?.close();
	}
});
}

test('tab close warns for pending, in-flight, and failed ordinary saves', async ({
	page,
	setupCode,
	browser
}) => {
	const seeded = await seed(page, browser, setupCode);
	const authorContext = await browser.newContext();
	const author = await signIn(authorContext, JORDAN, seeded.jordanResetCode, JORDAN_PASSWORD);
	let held: Awaited<ReturnType<typeof holdAppliedSave>> | null = null;
	let metadata: ReturnType<typeof gate> | null = null;
	try {
		await author.goto(seeded.draftUrl);
		await expect(author.getByLabel(MOST)).toBeVisible();
		await author.clock.pauseAt(new Date());
		const pendingText = 'Invented pending handover note.';
		await author.getByLabel(MOST).fill(pendingText);
		await expect(author.locator('.savestate')).toHaveText('Saving…');
		await dismissTabClose(author);
		await expect(author.getByLabel(MOST)).toHaveValue(pendingText);
		await author.clock.runFor(700);
		await expectSaved(author);
		await author.clock.resume();

		held = await holdAppliedSave(author, seeded.draftId);
		const inFlightText = 'Invented second handover note.';
		await author.getByLabel(MOST).fill(inFlightText);
		await held.first.arrived;
		await dismissTabClose(author);
		await expect(author.getByLabel(MOST)).toHaveValue(inFlightText);
		held.first.release();
		await expectSaved(author);
		await author.unroute(`**/api/drafts/${seeded.draftId}/content`);

		await author.route(`**/api/drafts/${seeded.draftId}/content`, async (route) => {
			await route.abort('failed');
		});
		await author.getByLabel(MOST).fill('Invented note from the failed save.');
		await expect(author.locator('.savestate')).toHaveText('Save failed');
		await dismissTabClose(author);
		await expect(author.getByLabel(MOST)).toHaveValue('Invented note from the failed save.');
		await author.unroute(`**/api/drafts/${seeded.draftId}/content`);

		const recovered = 'Invented note saved after the failure.';
		metadata = await holdDraftMetadata(author, seeded.draftId);
		await author.getByLabel(MOST).fill(recovered);
		await expectSaved(author);
		await metadata.arrived;
		await settleDraftMetadata(author, seeded.draftId, metadata);
		let cleanExitPrompts = 0;
		author.on('dialog', async (dialog) => {
			cleanExitPrompts += 1;
			await dialog.dismiss();
		});
		await author.close({ runBeforeUnload: true });
		expect(cleanExitPrompts, 'a settled save should close without a warning').toBe(0);
		await expect.poll(() => author.isClosed()).toBe(true);
		const revisit = await authorContext.newPage();
		await revisit.goto(seeded.draftUrl);
		await expect(revisit.getByLabel(MOST)).toHaveValue(recovered);
		await expect(revisit.locator('details.refused')).toHaveCount(0);
	} finally {
		metadata?.release();
		held?.first.release();
		await authorContext.close();
	}
});

test('a full-document link explains why pending text cannot leave', async ({
	page,
	setupCode,
	browser
}) => {
	const seeded = await seed(page, browser, setupCode);
	const authorContext = await browser.newContext();
	const author = await signIn(authorContext, JORDAN, seeded.jordanResetCode, JORDAN_PASSWORD);
	let metadata: ReturnType<typeof gate> | null = null;
	try {
		await author.goto(seeded.draftUrl);
		await expect(author.getByLabel(MOST)).toBeVisible();
		await author.clock.pauseAt(new Date());
		const sentence = 'Invented note awaiting its ordinary save.';
		await author.getByLabel(MOST).fill(sentence);
		await author.evaluate(() => {
			const link = document.createElement('a');
			link.id = 'full-document-link';
			link.href = '/api/health';
			link.setAttribute('data-sveltekit-reload', '');
			link.textContent = 'Open health check';
			document.body.appendChild(link);
		});
		await author.locator('#full-document-link').click();
		await expect(author).toHaveURL(new RegExp(`${seeded.draftUrl}$`));
		await expect(author.getByRole('alert')).toContainText('unsaved changes');
		await expect(author.getByLabel(MOST)).toHaveValue(sentence);
		metadata = await holdDraftMetadata(author, seeded.draftId);
		await author.clock.runFor(700);
		await expectSaved(author);
		await metadata.arrived;
		await settleDraftMetadata(author, seeded.draftId, metadata);
		await author.locator('#full-document-link').click();
		await expect(author).toHaveURL(/\/api\/health$/);
	} finally {
		metadata?.release();
		await authorContext.close();
	}
});

test('a failed winner reload still shows a refused narrative deletion', async ({
	page,
	setupCode,
	browser
}) => {
	const seeded = await seed(page, browser, setupCode);
	const baseline = 'Invented original narrative awaiting correction.';
	await page.goto(seeded.draftUrl);
	await page.getByLabel(MOST).fill(baseline);
	await page.getByLabel('Rate Emergency Call Interrogation').selectOption({ label: '4 — Meets standards' });
	await expectSaved(page);
	const authorContext = await browser.newContext();
	const author = await signIn(authorContext, CASEY, seeded.caseyResetCode, CASEY_PASSWORD);
	const losing = gate();
	try {
		await author.goto(seeded.draftUrl);
		await expect(author.getByLabel(MOST)).toHaveValue(baseline);
		await author.route(`**/api/drafts/${seeded.draftId}/content`, async (route) => {
			if (route.request().method() !== 'PUT') {
				await route.continue();
				return;
			}
			losing.arrive();
			await losing.released;
			await route.continue();
		});
		let refuseReload = false;
		await author.route(`**/api/drafts/${seeded.draftId}`, async (route) => {
			if (refuseReload && route.request().method() === 'GET' &&
				new URL(route.request().url()).pathname === `/api/drafts/${seeded.draftId}`) {
				await route.abort('failed');
				return;
			}
			await route.continue();
		});
		let finalizations = 0;
		await author.route(`**/api/drafts/${seeded.draftId}/finalize`, async (route) => {
			finalizations += 1;
			await route.continue();
		});
		await author.getByLabel(MOST).fill('');
		await author.getByLabel('Rate Emergency Call Interrogation').selectOption({ index: 0 });
		await losing.arrived;
		await page.getByLabel(LEAST).fill('Invented winning desk note.');
		await expectSaved(page);
		refuseReload = true;
		const refused = author.waitForResponse((response) =>
			response.url().includes(`/api/drafts/${seeded.draftId}/content`) &&
			response.status() === 409
		);
		losing.release();
		await refused;
		await expect(author.getByRole('alert')).toContainText('reloaded unsuccessfully');
		const deletion = author.locator('details.refused .narrative').filter({ hasText: MOST });
		await expect(deletion).toContainText('Clear this narrative');
		const clearedRating = author.locator('details.refused table.grid tbody tr').filter({
			hasText: 'Emergency Call Interrogation'
		});
		await expect(clearedRating.locator('td').nth(1)).toHaveText('—');
		await expect(author.getByRole('button', { name: 'Finalize record' })).toHaveCount(0);
		expect(finalizations).toBe(0);
	} finally {
		losing.release();
		await authorContext.close();
	}
});

test('a queued follow-up save keeps SPA departure on the draft until accepted', async ({
	page,
	setupCode,
	browser
}) => {
	const seeded = await seed(page, browser, setupCode);
	const authorContext = await browser.newContext();
	const author = await signIn(authorContext, JORDAN, seeded.jordanResetCode, JORDAN_PASSWORD);
	const second = gate();
	let held: Awaited<ReturnType<typeof holdAppliedSave>> | null = null;
	try {
		await author.goto(seeded.draftUrl);
		await expect(author.getByLabel(MOST)).toBeVisible();
		await markSpa(author);
		held = await holdAppliedSave(author, seeded.draftId, second);
		await author.getByLabel(MOST).fill('Invented first note already applied by the server.');
		await held.first.arrived;
		const later = 'Invented later note queued while the first save waits.';
		await author.getByLabel(MOST).fill(later);
		await author.getByRole('link', { name: 'Home' }).click();
		await expect(author).toHaveURL(new RegExp(`${seeded.draftUrl}$`));
		await expect(author.getByRole('alert')).toContainText('Try leaving again after it saves');
		await dismissTabClose(author);
		await expect(author.getByLabel(MOST)).toHaveValue(later);
		held.first.release();
		await second.arrived;
		await author.getByRole('link', { name: 'Home' }).click();
		await expect(author).toHaveURL(new RegExp(`${seeded.draftUrl}$`));
		await dismissTabClose(author);
		await expect(author.getByLabel(MOST)).toHaveValue(later);
		second.release();
		await expect.poll(async () => {
			const draft = await (await page.request.get(`/api/drafts/${seeded.draftId}`)).json();
			return draft.content.narratives.some((entry: { text: string }) => entry.text === later);
		}, { message: 'the queued follow-up edit did not finish' }).toBe(true);
		await expect.poll(() => held?.revisions.length).toBe(2);
		await author.waitForLoadState('networkidle');
		await expectUnloadReady(author);
		await homeSessions(author);
		let cleanExitPrompts = 0;
		author.on('dialog', async (dialog) => {
			cleanExitPrompts += 1;
			await dialog.dismiss();
		});
		await author.close({ runBeforeUnload: true });
		await expect.poll(() => author.isClosed()).toBe(true);
		expect(cleanExitPrompts, 'settled save chain should close without a warning').toBe(0);
	} finally {
		held?.first.release();
		second.release();
		await authorContext.close();
	}
});

test('a failed ordinary save cannot silently leave by SPA navigation', async ({
	page,
	setupCode,
	browser
}) => {
	const seeded = await seed(page, browser, setupCode);
	const authorContext = await browser.newContext();
	const author = await signIn(authorContext, JORDAN, seeded.jordanResetCode, JORDAN_PASSWORD);
	try {
		await author.goto(seeded.draftUrl);
		await expect(author.getByLabel(MOST)).toBeVisible();
		await author.route(`**/api/drafts/${seeded.draftId}/content`, async (route) => {
			await route.abort('failed');
		});
		const unsaved = 'Invented note whose save could not reach the server.';
		await author.getByLabel(MOST).fill(unsaved);
		await expect(author.locator('.savestate')).toHaveText('Save failed');
		await author.getByRole('link', { name: 'Home' }).click();
		await expect(author.getByRole('alert')).toContainText('Stay on this draft');
		await expect(author).toHaveURL(new RegExp(`${seeded.draftUrl}$`));
		await expect(author.getByLabel(MOST)).toHaveValue(unsaved);
		await author.unroute(`**/api/drafts/${seeded.draftId}/content`);
		await author.getByLabel(MOST).fill('Invented corrected note saved on retry.');
		await expectSaved(author);
		await expectUnloadReady(author);
		await markSpa(author);
		await homeSessions(author);
	} finally {
		await authorContext.close();
	}
});

test('an unchanged failed save can retry until it reaches the server', async ({
	page,
	setupCode,
	browser
}) => {
	const seeded = await seed(page, browser, setupCode);
	const authorContext = await browser.newContext();
	const author = await signIn(authorContext, JORDAN, seeded.jordanResetCode, JORDAN_PASSWORD);
	const thirdAttempt = gate();
	const bodies: string[] = [];
	try {
		await author.goto(seeded.draftUrl);
		await markSpa(author);
		const before = await (await page.request.get(`/api/drafts/${seeded.draftId}`)).json();
		await author.route(`**/api/drafts/${seeded.draftId}/content`, async (route) => {
			if (route.request().method() !== 'PUT') {
				await route.continue();
				return;
			}
			bodies.push(route.request().postData() ?? '');
			if (bodies.length <= 2) {
				await route.abort('failed');
				return;
			}
			thirdAttempt.arrive();
			await thirdAttempt.released;
			await route.continue();
		});
		const unsaved = 'Invented desk note awaiting restored connectivity.';
		await author.getByLabel(MOST).fill(unsaved);
		await expect(author.locator('.savestate')).toHaveText('Save failed');
		await expect.poll(() => bodies.length).toBe(1);
		await author.getByRole('link', { name: 'Home' }).click();
		await expect(author.getByRole('alert')).toContainText('Stay on this draft');
		await expect(author).toHaveURL(new RegExp(`${seeded.draftUrl}$`));
		const retry = author.getByRole('button', { name: 'Retry save' });
		await expect(retry).toBeVisible();
		await retry.click();
		await expect.poll(() => bodies.length).toBe(2);
		await expect(author.locator('.savestate')).toHaveText('Save failed');
		expect(bodies[1], 'retry must send the same unchanged snapshot').toBe(bodies[0]);
		await author.getByRole('link', { name: 'Home' }).click();
		await expect(author).toHaveURL(new RegExp(`${seeded.draftUrl}$`));
		await expect(author.getByLabel(MOST)).toHaveValue(unsaved);
		const stillUnsaved = await (await page.request.get(`/api/drafts/${seeded.draftId}`)).json();
		expect(stillUnsaved.revision).toBe(before.revision);
		await retry.click();
		await thirdAttempt.arrived;
		expect(bodies[2], 'restored connectivity must send the same unchanged snapshot').toBe(bodies[0]);
		expect(JSON.parse(bodies[2]).revision).toBe(before.revision);
		await author.getByRole('link', { name: 'Home' }).click();
		await expect(author).toHaveURL(new RegExp(`${seeded.draftUrl}$`));
		await expect(author.getByRole('alert')).toContainText('Try leaving again after it saves');
		thirdAttempt.release();
		await expectSaved(author);
		await expectUnloadReady(author);
		const saved = await (await page.request.get(`/api/drafts/${seeded.draftId}`)).json();
		expect(saved.revision).toBe(before.revision + 1);
		expect(saved.content.narratives.some((entry: { text: string }) => entry.text === unsaved))
			.toBe(true);
		await homeSessions(author);
	} finally {
		thirdAttempt.release();
		await authorContext.close();
	}
});

test('retrying a failed save against an advanced revision preserves its refused text', async ({
	page,
	setupCode,
	browser
}) => {
	const seeded = await seed(page, browser, setupCode);
	const authorContext = await browser.newContext();
	const author = await signIn(authorContext, CASEY, seeded.caseyResetCode, CASEY_PASSWORD);
	let failTransport = true;
	let saves = 0;
	let submissions = 0;
	const saveStatuses: number[] = [];
	try {
		await author.goto(seeded.draftUrl);
		await expect(author.getByRole('button', { name: 'Submit for review' })).toBeVisible();
		author.on('response', (response) => {
			if (response.request().method() === 'PUT' &&
				new URL(response.url()).pathname === `/api/drafts/${seeded.draftId}/content`) {
				saveStatuses.push(response.status());
			}
		});
		await author.route(`**/api/drafts/${seeded.draftId}/content`, async (route) => {
			if (route.request().method() !== 'PUT') {
				await route.continue();
				return;
			}
			saves += 1;
			if (failTransport) {
				await route.abort('failed');
				return;
			}
			await route.continue();
		});
		await author.route(`**/api/drafts/${seeded.draftId}/submit`, async (route) => {
			submissions += 1;
			await route.continue();
		});
		const refused = 'Invented note held after a failed network request.';
		await author.getByLabel(MOST).fill(refused);
		await expect(author.locator('.savestate')).toHaveText('Save failed');
		await expect.poll(() => saves).toBe(1);
		await page.goto(seeded.draftUrl);
		const winner = 'Invented winning note saved while the desk was disconnected.';
		await page.getByLabel(MOST).fill(winner);
		await expectSaved(page);
		const winning = await (await page.request.get(`/api/drafts/${seeded.draftId}`)).json();
		failTransport = false;
		const retry = author.getByRole('button', { name: 'Retry save' });
		await expect(retry).toBeVisible();
		await retry.click();
		await expect.poll(() => saves).toBe(2);
		await expect.poll(() => saveStatuses).toContain(409);
		await expect(refusedText(author, MOST)).toHaveText(refused);
		await expect(author.getByLabel(MOST)).toHaveValue(winner);
		expect(saves, 'refusal must not resubmit the losing snapshot').toBe(2);
		const after = await (await page.request.get(`/api/drafts/${seeded.draftId}`)).json();
		expect(after.revision).toBe(winning.revision);
		expect(after.content.narratives.some((entry: { text: string }) => entry.text === winner))
			.toBe(true);
		const submit = author.getByRole('button', { name: 'Submit for review' });
		await expect(submit).toBeVisible();
		await submit.click();
		await expect(author.getByRole('alert')).toContainText('Your refused text is still here');
		expect(submissions, 'workflow must wait for the writer to resolve refused text').toBe(0);
	} finally {
		await authorContext.close();
	}
});

test('a first save refused by a frozen draft survives a failed reload', async ({
	page,
	setupCode,
	browser
}) => {
	const seeded = await seed(page, browser, setupCode);
	const authorContext = await browser.newContext();
	const author = await signIn(authorContext, JORDAN, seeded.jordanResetCode, JORDAN_PASSWORD);
	const reload = gate();
	let reloads = 0;
	let puts = 0;
	try {
		await author.goto(seeded.draftUrl);
		await expect(author.getByLabel(MOST)).toBeEnabled();
		await markSpa(author);
		const initial = await (await page.request.get(`/api/drafts/${seeded.draftId}`)).json();
		const submit = await page.request.post(`/api/drafts/${seeded.draftId}/submit`, {
			data: { revision: initial.revision }
		});
		expect(submit.ok(), `freeze failed: ${submit.status()} ${await submit.text()}`).toBe(true);
		await author.route(`**/api/drafts/${seeded.draftId}`, async (route) => {
			if (route.request().method() !== 'GET' ||
				new URL(route.request().url()).pathname !== `/api/drafts/${seeded.draftId}`) {
				await route.continue();
				return;
			}
			reloads += 1;
			if (reloads === 1) {
				reload.arrive();
				await reload.released;
				await route.abort('failed');
				return;
			}
			await route.continue();
		});
		author.on('request', (request) => {
			if (request.method() === 'PUT' &&
				new URL(request.url()).pathname === `/api/drafts/${seeded.draftId}/content`) {
				puts += 1;
			}
		});
		const local = 'Invented dispatch note typed before the frozen status arrived.';
		const denied = author.waitForResponse((response) =>
			response.request().method() === 'PUT' &&
			new URL(response.url()).pathname === `/api/drafts/${seeded.draftId}/content` &&
			response.status() === 409
		);
		await author.getByLabel(MOST).fill(local);
		expect((await (await denied).json()).error).toBe('draft_submitted');
		await expect.poll(() => reloads, {
			message: 'a frozen refusal must reload the authoritative draft'
		}).toBe(1);
		await reload.arrived;
		await author.getByRole('link', { name: 'Home' }).click();
		await expect(author).toHaveURL(new RegExp(`${seeded.draftUrl}$`));
		await expect(author.getByRole('alert')).toContainText('latest draft is still loading');
		reload.release();
		await expect(author.getByRole('alert')).toContainText('could not load');
		await expect(refusedText(author, MOST)).toHaveText(local);
		await expect(author.getByRole('button', { name: 'Reload latest draft' })).toBeVisible();
		await author.getByRole('button', { name: 'Reload latest draft' }).click();
		await expect(author.getByText('Submitted for review', { exact: true })).toBeVisible();
		await expect(author.getByLabel(MOST)).toHaveValue('');
		await expect(author.getByLabel(MOST)).toBeDisabled();
		await expect(refusedText(author, MOST)).toHaveText(local);
		await expect(author.locator('details.refused')).toContainText('cannot be saved to this draft');
		expect(puts, 'the refused copy must not be sent again').toBe(1);
		await author.getByRole('button', { name: 'Discard this text' }).click();
		await expect(author.locator('details.refused')).toHaveCount(0);
		await homeSessions(author);
		const frozen = await (await page.request.get(`/api/drafts/${seeded.draftId}`)).json();
		expect(frozen.status).toBe('submitted');
		expect(frozen.content.narratives).toEqual([]);
	} finally {
		reload.release();
		await authorContext.close();
	}
});

for (const outcome of ['typed frozen refusal', 'transport failure on a frozen view'] as const) {
test(`a queued save with ${outcome} keeps its text and permits an exit`, async ({
	page,
	setupCode,
	browser
}) => {
	const seeded = await seed(page, browser, setupCode);
	const authorContext = await browser.newContext();
	const author = await signIn(authorContext, JORDAN, seeded.jordanResetCode, JORDAN_PASSWORD);
	const second = gate();
	const recovery = gate();
	let draftGets = 0;
	let held: Awaited<ReturnType<typeof holdAppliedSave>> | null = null;
	try {
		await author.goto(seeded.draftUrl);
		await expect(author.getByLabel(MOST)).toBeEnabled();
		await markSpa(author);
		held = await holdAppliedSave(
			author, seeded.draftId, second, outcome === 'transport failure on a frozen view'
		);
		await author.route(`**/api/drafts/${seeded.draftId}`, async (route) => {
			if (route.request().method() !== 'GET' ||
				new URL(route.request().url()).pathname !== `/api/drafts/${seeded.draftId}`) {
				await route.continue();
				return;
			}
			draftGets += 1;
			if (draftGets === 2) {
				recovery.arrive();
				await recovery.released;
			}
			await route.continue();
		});
		const applied = 'Invented first note accepted before submission.';
		await author.getByLabel(MOST).fill(applied);
		await held.first.arrived;
		const first = await (await page.request.get(`/api/drafts/${seeded.draftId}`)).json();
		expect(first.content.narratives.some((entry: { text: string }) => entry.text === applied))
			.toBe(true);
		const local = 'Invented later note that the frozen draft refused.';
		await author.getByLabel(MOST).fill(local);
		const submit = await page.request.post(`/api/drafts/${seeded.draftId}/submit`, {
			data: { revision: first.revision }
		});
		expect(submit.ok(), `freeze failed: ${submit.status()} ${await submit.text()}`).toBe(true);
		// The real server is frozen; in the transport variant, the browser
		// loses B's reply before receiving the typed refusal. Metadata from
		// A has already told this controller the view is read-only.
		const denied = outcome === 'typed frozen refusal'
			? author.waitForResponse((response) =>
				response.request().method() === 'PUT' &&
				new URL(response.url()).pathname === `/api/drafts/${seeded.draftId}/content` &&
				response.status() === 409
			)
			: null;
		const failed = outcome === 'transport failure on a frozen view'
			? author.waitForEvent('requestfailed', (request) =>
				request.method() === 'PUT' &&
				new URL(request.url()).pathname === `/api/drafts/${seeded.draftId}/content`
			)
			: null;
		held.first.release();
		await second.arrived;
		await expect(author.getByText('Submitted for review', { exact: true })).toBeVisible();
		await expect(author.getByText('Local copy awaiting server comparison')).toBeVisible();
		await expect(author.getByText("These fields do not establish the server's current content.")).toBeVisible();
		await expect(author.getByLabel(MOST)).toHaveValue(local);
		await expect(author.getByLabel(MOST)).toHaveAttribute('readonly');
		second.release();
		if (outcome === 'typed frozen refusal') {
			expect((await (await denied!).json()).error).toBe('draft_submitted');
		} else {
			await failed!;
		}
		expect(held.revisions).toHaveLength(2);
		expect(held.revisions[1]).toBe(first.revision);
		await recovery.arrived;
		expect(draftGets).toBe(2);
		await expect(author.getByText('Local copy awaiting server comparison')).toBeVisible();
		await expect(author.getByLabel(MOST)).toHaveValue(local);
		await expect(author.getByLabel(MOST)).toHaveAttribute('readonly');
		recovery.release();
		await expect(refusedText(author, MOST)).toHaveText(local);
		await expect(author.getByText('Local copy awaiting server comparison')).toHaveCount(0);
		await expect(author.getByText('Submitted for review', { exact: true })).toBeVisible();
		await expect(author.getByLabel(MOST)).toHaveValue(applied);
		await expect(author.getByLabel(MOST)).toBeDisabled();
		await expect(author.locator('details.refused')).toContainText('cannot be saved to this draft');
		await expect(author.getByRole('button', { name: 'Retry save' })).toHaveCount(0);
		const frozen = await (await page.request.get(`/api/drafts/${seeded.draftId}`)).json();
		expect(frozen.status).toBe('submitted');
		expect(frozen.content.narratives.some((entry: { text: string }) => entry.text === applied))
			.toBe(true);
		expect(frozen.content.narratives.some((entry: { text: string }) => entry.text === local))
			.toBe(false);
		await author.getByRole('button', { name: 'Discard this text' }).click();
		await expect(author.locator('details.refused')).toHaveCount(0);
		await homeSessions(author);
	} finally {
		held?.first.release();
		second.release();
		recovery.release();
		await authorContext.close();
	}
});
}

test('a queued save frozen by finalization keeps a local copy until the sealed record loads', async ({
	page,
	setupCode,
	browser
}) => {
	const seeded = await seed(page, browser, setupCode);
	const authorContext = await browser.newContext();
	const coordinatorContext = await browser.newContext();
	const author = await signIn(authorContext, JORDAN, seeded.jordanResetCode, JORDAN_PASSWORD);
	const coordinator = await signIn(
		coordinatorContext, CASEY, seeded.caseyResetCode, CASEY_PASSWORD
	);
	const second = gate();
	const recovery = gate();
	let draftGets = 0;
	let held: Awaited<ReturnType<typeof holdAppliedSave>> | null = null;
	try {
		await author.goto(seeded.draftUrl);
		await expect(author.getByLabel(MOST)).toBeEnabled();
		await markSpa(author);
		held = await holdAppliedSave(author, seeded.draftId, second);
		await author.route(`**/api/drafts/${seeded.draftId}`, async (route) => {
			if (route.request().method() !== 'GET' ||
				new URL(route.request().url()).pathname !== `/api/drafts/${seeded.draftId}`) {
				await route.continue();
				return;
			}
			draftGets += 1;
			if (draftGets === 2) {
				recovery.arrive();
				await recovery.released;
			}
			await route.continue();
		});
		const applied = 'Invented note accepted before finalization.';
		const local = 'Invented later note refused by the finalized record.';
		await author.getByLabel(MOST).fill(applied);
		await held.first.arrived;
		const first = await (await page.request.get(`/api/drafts/${seeded.draftId}`)).json();
		await author.getByLabel(MOST).fill(local);
		const finalized = await coordinator.request.post(`/api/drafts/${seeded.draftId}/finalize`, {
			data: { revision: first.revision }
		});
		expect(finalized.ok(), `finalization failed: ${finalized.status()} ${await finalized.text()}`)
			.toBe(true);
		const denied = author.waitForResponse((response) =>
			response.request().method() === 'PUT' &&
			new URL(response.url()).pathname === `/api/drafts/${seeded.draftId}/content` &&
			response.status() === 409
		);
		held.first.release();
		await second.arrived;
		await expect(author.getByText('Finalized', { exact: true })).toBeVisible();
		await expect(author.getByText('Local copy awaiting server comparison')).toBeVisible();
		await expect(author.getByLabel(MOST)).toHaveValue(local);
		await expect(author.getByLabel(MOST)).toHaveAttribute('readonly');
		second.release();
		expect((await (await denied).json()).error).toBe('draft_finalized');
		await recovery.arrived;
		expect(draftGets).toBe(2);
		await expect(author.getByLabel(MOST)).toHaveValue(local);
		await expect(author.getByText('Local copy awaiting server comparison')).toBeVisible();
		recovery.release();
		await expect(refusedText(author, MOST)).toHaveText(local);
		await expect(author.getByText('Local copy awaiting server comparison')).toHaveCount(0);
		await expect(author.getByRole('heading', { name: 'Finalized record' })).toBeVisible();
		await expect(author.getByLabel(MOST)).toHaveCount(0);
		await expect(author.locator('.sealed-text').filter({ hasText: applied })).toBeVisible();
		const server = await (await page.request.get(`/api/drafts/${seeded.draftId}`)).json();
		expect(server.status).toBe('finalized');
		expect(server.content.narratives.some((entry: { text: string }) => entry.text === local))
			.toBe(false);
		await author.getByRole('button', { name: 'Discard this text' }).click();
		await homeSessions(author);
	} finally {
		held?.first.release();
		second.release();
		recovery.release();
		await authorContext.close();
		await coordinatorContext.close();
	}
});

test('a reviewer waits for frozen local recovery text before deciding', async ({
	page,
	setupCode,
	browser
}) => {
	const seeded = await seed(page, browser, setupCode);
	const reviewerContext = await browser.newContext();
	const reviewer = await signIn(
		reviewerContext, CASEY, seeded.caseyResetCode, CASEY_PASSWORD
	);
	let reviews = 0;
	try {
		await reviewer.goto(seeded.draftUrl);
		await expect(reviewer.getByLabel(MOST)).toBeEnabled();
		const initial = await (await page.request.get(`/api/drafts/${seeded.draftId}`)).json();
		const submit = await page.request.post(`/api/drafts/${seeded.draftId}/submit`, {
			data: { revision: initial.revision }
		});
		expect(submit.ok(), `submission failed: ${submit.status()} ${await submit.text()}`)
			.toBe(true);
		reviewer.on('request', (request) => {
			if (request.method() === 'POST' &&
				new URL(request.url()).pathname === `/api/drafts/${seeded.draftId}/review`) {
				reviews += 1;
			}
		});
		const local = 'Invented reviewer note refused by the submitted draft.';
		const denied = reviewer.waitForResponse((response) =>
			response.request().method() === 'PUT' &&
			new URL(response.url()).pathname === `/api/drafts/${seeded.draftId}/content` &&
			response.status() === 409
		);
		await reviewer.getByLabel(MOST).fill(local);
		expect((await (await denied).json()).error).toBe('draft_submitted');
		await expect(refusedText(reviewer, MOST)).toHaveText(local);
		await expect(reviewer.getByText('Submitted for review', { exact: true })).toBeVisible();
		const eligible = await (await reviewer.request.get(`/api/drafts/${seeded.draftId}`)).json();
		expect(eligible.viewer_may_review, 'reviewer must have an actual review action')
			.toBe(true);
		await expect(reviewer.getByRole('button', { name: 'Decide' })).toHaveCount(0);
		expect(reviews, 'no review may run while the local buffer is unresolved').toBe(0);
		await reviewer.getByRole('button', { name: 'Discard this text' }).click();
		const decide = reviewer.getByRole('button', { name: 'Decide' });
		await expect(decide).toBeVisible();
		const reviewed = reviewer.waitForResponse((response) =>
			response.request().method() === 'POST' &&
			new URL(response.url()).pathname === `/api/drafts/${seeded.draftId}/review`
		);
		await decide.click();
		expect((await reviewed).ok()).toBe(true);
		expect(reviews).toBe(1);
		const decided = await (await page.request.get(`/api/drafts/${seeded.draftId}`)).json();
		expect(decided.status).toBe('approved');
	} finally {
		await reviewerContext.close();
	}
});

test('a frozen failed save permits deliberate SPA departure after recovery', async ({
	page,
	setupCode,
	browser
}) => {
	const seeded = await seed(page, browser, setupCode);
	const authorContext = await browser.newContext();
	const author = await signIn(authorContext, JORDAN, seeded.jordanResetCode, JORDAN_PASSWORD);
	let held: Awaited<ReturnType<typeof holdAppliedSave>> | null = null;
	try {
		await author.goto(seeded.draftUrl);
		await expect(author.getByLabel(MOST)).toBeEnabled();
		await markSpa(author);
		held = await holdAppliedSave(author, seeded.draftId);
		await author.getByLabel(MOST).fill('Invented accepted note before the draft froze.');
		await held.first.arrived;
		const applied = await (await page.request.get(`/api/drafts/${seeded.draftId}`)).json();
		await author.getByLabel(MOST).fill('Invented local note withheld by the frozen draft.');
		const submit = await page.request.post(`/api/drafts/${seeded.draftId}/submit`, {
			data: { revision: applied.revision }
		});
		expect(submit.ok(), `freeze failed: ${submit.status()} ${await submit.text()}`).toBe(true);
		const denied = author.waitForResponse((response) =>
			response.request().method() === 'PUT' &&
			new URL(response.url()).pathname === `/api/drafts/${seeded.draftId}/content` &&
			response.status() === 409
		);
		held.first.release();
		expect((await (await denied).json()).error).toBe('draft_submitted');
		await expect(author.getByText('Submitted for review', { exact: true })).toBeVisible();
		await author.waitForLoadState('networkidle');
		await expect(refusedText(author, MOST)).toHaveText(
			'Invented local note withheld by the frozen draft.'
		);
		await author.getByRole('link', { name: 'Home' }).click();
		await expect(author).toHaveURL(/\/$/);
		await expectSpaAlive(author);
		await expect(author.locator('details.refused')).toHaveCount(0);
	} finally {
		held?.first.release();
		await authorContext.close();
	}
});

test('recovery and discard release a full-document link after a stale save', async ({
	page,
	setupCode,
	browser
}) => {
	const seeded = await seed(page, browser, setupCode);
	const authorContext = await browser.newContext();
	const author = await signIn(authorContext, JORDAN, seeded.jordanResetCode, JORDAN_PASSWORD);
	const losing = gate();
	try {
		await author.goto(seeded.draftUrl);
		await expect(author.getByLabel(MOST)).toBeVisible();
		await author.clock.pauseAt(new Date());
		await author.route(`**/api/drafts/${seeded.draftId}/content`, async (route) => {
			if (route.request().method() !== 'PUT') {
				await route.continue();
				return;
			}
			losing.arrive();
			await losing.released;
			await route.continue();
		});
		await author.getByLabel(MOST).fill('Invented refused first note.');
		await author.clock.runFor(700);
		await losing.arrived;
		await author.getByLabel(LEAST).fill('Invented later note typed while the request waited.');
		await author.clock.runFor(700);
		await page.goto(seeded.draftUrl);
		await page.getByLabel(MOST).fill('Invented winning note.');
		await expectSaved(page);
		losing.release();
		await expect(author.locator('details.refused')).toBeVisible();
		await author.getByRole('button', { name: 'Discard this text' }).click();
		await expect(author.locator('details.refused')).toHaveCount(0);
		await author.evaluate(() => {
			const link = document.createElement('a');
			link.id = 'leave-after-discard';
			link.href = '/api/health';
			link.setAttribute('data-sveltekit-reload', '');
			link.textContent = 'Open health check';
			document.body.appendChild(link);
		});
		await author.locator('#leave-after-discard').click();
		await expect(author).toHaveURL(/\/api\/health$/);
	} finally {
		losing.release();
		await authorContext.close();
	}
});

test('a failed save during SPA departure keeps the ordinary edit on its draft', async ({
	page,
	setupCode,
	browser
}) => {
	const seeded = await seed(page, browser, setupCode);
	const authorContext = await browser.newContext();
	const author = await signIn(authorContext, JORDAN, seeded.jordanResetCode, JORDAN_PASSWORD);
	const save = gate();
	try {
		await author.goto(seeded.draftUrl);
		await markSpa(author);
		await author.route(`**/api/drafts/${seeded.draftId}/content`, async (route) => {
			if (route.request().method() !== 'PUT') {
				await route.continue();
				return;
			}
			save.arrive();
			await save.released;
			await route.abort('failed');
		});
		const unsaved = 'Invented handover text from a disconnected desk.';
		await author.getByLabel(MOST).fill(unsaved);
		await save.arrived;
		await author.getByRole('link', { name: 'Home' }).click();
		save.release();
		await expect(author.locator('.savestate')).toHaveText('Save failed');
		await expect(author).toHaveURL(new RegExp(`${seeded.draftUrl}$`));
		await expect(author.getByRole('alert')).toContainText('did not save');
		await expect(author.getByLabel(MOST)).toHaveValue(unsaved);
		await author.unroute(`**/api/drafts/${seeded.draftId}/content`);
		await author.getByLabel(MOST).fill('Invented handover text saved on retry.');
		await expectSaved(author);
		await expectUnloadReady(author);
		await homeSessions(author);
		const draft = await (await page.request.get(`/api/drafts/${seeded.draftId}`)).json();
		expect(draft.content.narratives.some((entry: { text: string }) =>
			entry.text === 'Invented handover text saved on retry.'
		)).toBe(true);
	} finally {
		save.release();
		await authorContext.close();
	}
});

test('a failed winner reload can retry in place and preserve a refused deletion', async ({
	page,
	setupCode,
	browser
}) => {
	const seeded = await seed(page, browser, setupCode);
	const original = 'Invented initial note awaiting a correction.';
	await page.goto(seeded.draftUrl);
	await page.getByLabel(MOST).fill(original);
	await expectSaved(page);
	const authorContext = await browser.newContext();
	const author = await signIn(authorContext, CASEY, seeded.caseyResetCode, CASEY_PASSWORD);
	const losing = gate();
	let failReload = false;
	let puts = 0;
	let finalizations = 0;
	try {
		await author.goto(seeded.draftUrl);
		await author.route(`**/api/drafts/${seeded.draftId}/content`, async (route) => {
			if (route.request().method() === 'PUT') {
				puts += 1;
				if (puts === 1) {
					losing.arrive();
					await losing.released;
				}
			}
			await route.continue();
		});
		await author.route(`**/api/drafts/${seeded.draftId}`, async (route) => {
			if (failReload && route.request().method() === 'GET' &&
				new URL(route.request().url()).pathname === `/api/drafts/${seeded.draftId}`) {
				await route.abort('failed');
				return;
			}
			await route.continue();
		});
		await author.route(`**/api/drafts/${seeded.draftId}/finalize`, async (route) => {
			finalizations += 1;
			await route.continue();
		});
		await author.getByLabel(MOST).fill('');
		await losing.arrived;
		const winning = 'Invented newer note from the other desk.';
		await page.getByLabel(MOST).fill(winning);
		await expectSaved(page);
		failReload = true;
		losing.release();
		await expect(author.getByRole('alert')).toContainText('reloaded unsuccessfully');
		await expect(author.locator('details.refused')).toContainText('Clear this narrative');
		const reload = author.getByRole('button', { name: 'Reload latest draft' });
		await expect(reload).toBeVisible();
		failReload = false;
		await reload.click();
		await expect(author.getByLabel(MOST)).toHaveValue(winning);
		await expect(author.locator('details.refused')).toContainText('Clear this narrative');
		expect(puts, 'retrying the reload must not save the refused deletion').toBe(1);
		await author.getByRole('button', { name: 'Finalize record' }).click();
		await expect(author.getByRole('alert')).toContainText('Your refused text is still here');
		expect(finalizations).toBe(0);
		await author.getByLabel(MOST).fill('');
		await expectSaved(author);
		expect(puts).toBe(2);
		const finalized = author.waitForResponse((response) =>
			response.url().includes(`/api/drafts/${seeded.draftId}/finalize`)
		);
		await author.getByRole('button', { name: 'Finalize record' }).click();
		expect((await finalized).ok()).toBe(true);
	} finally {
		losing.release();
		await authorContext.close();
	}
});

test('an allowed SPA departure locks clean draft fields until the route settles', async ({
	page,
	setupCode,
	browser
}) => {
	const seeded = await seed(page, browser, setupCode);
	const authorContext = await browser.newContext();
	const author = await signIn(authorContext, JORDAN, seeded.jordanResetCode, JORDAN_PASSWORD);
	const layout = gate();
	try {
		await author.goto(seeded.draftUrl);
		await markSpa(author);
		await expect(author.getByLabel(MOST)).toBeEnabled();
		await author.route('**/api/instance', async (route) => {
			const response = await route.fetch();
			if (!response.ok()) throw new Error(`layout refresh failed: ${response.status()}`);
			layout.arrive();
			await layout.released;
			await route.fulfill({ response });
		});
		const leaving = author.getByRole('link', { name: 'Home' }).click();
		await layout.arrived;
		await expect(author.getByLabel(MOST)).toBeDisabled();
		layout.release();
		await leaving;
		await expect(author.getByRole('heading', { name: 'My sessions' })).toBeVisible();
		await expectSpaAlive(author);
	} finally {
		layout.release();
		await authorContext.close();
	}
});

test('a refused save stays on its draft until the writer sees recovery', async ({
	page,
	setupCode,
	browser
}) => {
	const seeded = await seed(page, browser, setupCode);
	const authorContext = await browser.newContext();
	const author = await signIn(authorContext, CASEY, seeded.caseyResetCode, CASEY_PASSWORD);
	const losing = gate();
	const reload = gate();
	try {
		await author.goto(seeded.draftUrl);
		await expect(author.getByLabel(MOST)).toBeVisible();
		await markSpa(author);
		await author.route(`**/api/drafts/${seeded.draftId}/content`, async (route) => {
			if (route.request().method() !== 'PUT') {
				await route.continue();
				return;
			}
			losing.arrive();
			await losing.released;
			await route.continue();
		});
		await author.route(`**/api/drafts/${seeded.draftId}`, async (route) => {
			if (route.request().method() !== 'GET' ||
				new URL(route.request().url()).pathname !== `/api/drafts/${seeded.draftId}`) {
				await route.continue();
				return;
			}
			const response = await route.fetch();
			reload.arrive();
			await reload.released;
			await route.fulfill({ response });
		});
		const refused = 'Invented source note refused after another desk saved.';
		await author.getByLabel(MOST).fill(refused);
		await losing.arrived;
		await page.goto(seeded.draftUrl);
		await page.getByLabel(MOST).fill('Invented winning note from the other desk.');
		await expectSaved(page);
		await author.getByRole('link', { name: 'Home' }).click();
		await expect(author.getByRole('alert')).toContainText('Try leaving again after it saves');
		await expect(author).toHaveURL(new RegExp(`${seeded.draftUrl}$`));
		losing.release();
		await reload.arrived;
		await author.getByRole('link', { name: 'Home' }).click();
		await expect(author.getByRole('alert')).toContainText('latest draft is still loading');
		await expect(author).toHaveURL(new RegExp(`${seeded.draftUrl}$`));
		await dismissTabClose(author);
		reload.release();
		await expect(refusedText(author, MOST)).toHaveText(refused);
		await author.getByRole('link', { name: 'Home' }).click();
		await expect(author).toHaveURL(/\/$/);
		await expectSpaAlive(author);
		await expect(author.locator('details.refused')).toHaveCount(0);
	} finally {
		losing.release();
		reload.release();
		await authorContext.close();
	}
});

test('overlapping SPA navigation keeps the draft locked until the current route settles', async ({
	page,
	setupCode,
	browser
}) => {
	const seeded = await seed(page, browser, setupCode);
	const authorContext = await browser.newContext();
	const author = await signIn(authorContext, JORDAN, seeded.jordanResetCode, JORDAN_PASSWORD);
	const firstLayout = gate();
	try {
		await author.goto(seeded.draftUrl);
		await expect(author.getByLabel(MOST)).toBeVisible();
		await markSpa(author);
		await homeSessions(author);
		await openDraftFromHome(author, '2026-06-02', seeded.draftId);
		const entryCount = await author.evaluate(() => history.length);
		let instances = 0;
		await author.route('**/api/instance', async (route) => {
			instances += 1;
			const held = instances === 1 ? firstLayout : null;
			if (held !== null) {
				const response = await route.fetch();
				held.arrive();
				await held.released;
				try { await route.fulfill({ response }); } catch { /* superseded */ }
				return;
			}
			await route.continue();
		});
		void author.getByRole('link', { name: 'Home' }).click();
		await firstLayout.arrived;
		await expect(author.getByLabel(MOST)).toBeDisabled();
		// Real SPA entries exist for Home and this draft. Back starts a
		// second navigation to the same Home route (which can reuse the
		// first load); Forward returns to the current entry before commit.
		await author.evaluate(() => history.back());
		await expect(author).toHaveURL(/\/$/);
		await author.evaluate(() => history.forward());
		await expect(author).toHaveURL(new RegExp(`${seeded.draftUrl}$`));
		firstLayout.release();
		await author.waitForLoadState('networkidle');
		await expect(author).toHaveURL(new RegExp(`${seeded.draftUrl}$`));
		await expect(author.getByLabel(MOST)).toBeEnabled();
		await expect(author).toHaveURL(new RegExp(`${seeded.draftUrl}$`));
		expect(await author.evaluate(() => history.length)).toBe(entryCount);
		await expect(author.getByRole('button', { name: 'Resume editing this draft' })).toHaveCount(0);
		await expectSpaAlive(author);
	} finally {
		firstLayout.release();
		await authorContext.close();
	}
});

test('a stranded router state requires an explicit same-entry resume', async ({
	page,
	setupCode,
	browser
}) => {
	const seeded = await seed(page, browser, setupCode);
	const authorContext = await browser.newContext();
	const author = await signIn(authorContext, JORDAN, seeded.jordanResetCode, JORDAN_PASSWORD);
	const layout = gate();
	try {
		await author.goto(seeded.draftUrl);
		await expect(author.getByLabel(MOST)).toBeVisible();
		await markSpa(author);
		await homeSessions(author);
		await openDraftFromHome(author, '2026-06-02', seeded.draftId);
		const originalState = await author.evaluate(() => history.state);
		// Recreate a shallow popstate that resets the router token while
		// the destination GET is held. Native hash entries need the active
		// SvelteKit state copied onto them for this synthetic boundary.
		await author.evaluate(() => { location.hash = 'earlier'; });
		await expect(author).toHaveURL(/#earlier$/);
		await author.evaluate((state) => history.replaceState(state, '', location.href), originalState);
		await author.evaluate(() => { location.hash = 'current'; });
		await expect(author).toHaveURL(/#current$/);
		await author.evaluate((state) => history.replaceState(state, '', location.href), originalState);
		const entryCount = await author.evaluate(() => history.length);
		await author.route('**/api/instance', async (route) => {
			const response = await route.fetch();
			layout.arrive();
			await layout.released;
			try { await route.fulfill({ response }); } catch { /* abandoned navigation */ }
		});
		void author.getByRole('link', { name: 'Home' }).click();
		await layout.arrived;
		await expect(author.getByLabel(MOST)).toBeDisabled();
		await author.evaluate(() => history.back());
		await expect(author).toHaveURL(/#earlier$/);
		await author.evaluate(() => history.forward());
		await expect(author).toHaveURL(/#current$/);
		layout.release();
		await author.waitForLoadState('networkidle');
		await expect(author.getByLabel(MOST)).toBeDisabled();
		await expect(author.getByRole('button', { name: 'Resume editing this draft' })).toBeVisible();
		await author.getByRole('button', { name: 'Resume editing this draft' }).click();
		await expect(author.getByLabel(MOST)).toBeEnabled();
		await expect(author).toHaveURL(new RegExp(`${seeded.draftUrl}#current$`));
		expect(await author.evaluate(() => history.length)).toBe(entryCount);
		expect(await author.evaluate(() => history.state['sveltekit:states']))
			.toEqual(originalState['sveltekit:states']);
		const unsaved = 'Invented post-abort note.';
		await author.route(`**/api/drafts/${seeded.draftId}/content`, async (route) => {
			await route.abort('failed');
		});
		await author.getByLabel(MOST).fill(unsaved);
		await author.getByRole('link', { name: 'Home' }).click();
		await expect(author.locator('.savestate')).toHaveText('Save failed');
		await expect(author).toHaveURL(new RegExp(`${seeded.draftUrl}#current$`));
		await expect(author.getByLabel(MOST)).toHaveValue(unsaved);
	} finally {
		layout.release();
		await authorContext.close();
	}
});

for (const action of ['add', 'remove'] as const) {
	test(`a refused weekly save holds ${action} link until the winning copy loads`, async ({
		page,
		setupCode,
		browser
	}) => {
		const seeded = await seedWeekly(page, browser, setupCode);
		if (action === 'remove') {
			const summary = await (await page.request.get(`/api/drafts/${seeded.summaryId}`)).json();
			const linked = await page.request.post(`/api/drafts/${seeded.summaryId}/links`, {
				data: { daily_version_id: seeded.dailyVersionId, revision: summary.revision }
			});
			if (!linked.ok()) throw new Error(`seed link failed: ${linked.status()} ${await linked.text()}`);
		}
		const loserContext = seeded.coordinatorContext;
		const loser = seeded.coordinator;
		const losing = gate();
		const reload = gate();
		try {
			await loser.goto(seeded.summaryUrl);
			await expect(loser.getByLabel('Weekly overview.')).toBeVisible();
			if (action === 'add') {
				await loser.getByLabel('Link a daily report').selectOption(String(seeded.dailyVersionId));
			} else {
				await expect(loser.getByRole('button', { name: 'Remove' })).toBeVisible();
			}
			await loser.route(`**/api/drafts/${seeded.summaryId}/content`, async (route) => {
				if (route.request().method() !== 'PUT') {
					await route.continue();
					return;
				}
				losing.arrive();
				await losing.released;
				await route.continue();
			});
			let reloads = 0;
			await loser.route(`**/api/drafts/${seeded.summaryId}`, async (route) => {
				if (route.request().method() !== 'GET' ||
					new URL(route.request().url()).pathname !== `/api/drafts/${seeded.summaryId}`) {
					await route.continue();
					return;
				}
				reloads += 1;
				if (reloads === 1) {
					const response = await route.fetch();
					reload.arrive();
					await reload.released;
					await route.fulfill({ response });
					return;
				}
				await route.continue();
			});
			let linkMutations = 0;
			loser.on('request', (request) => {
				const path = new URL(request.url()).pathname;
				if (request.method() === 'POST' &&
					(path === `/api/drafts/${seeded.summaryId}/links` ||
						path === `/api/drafts/${seeded.summaryId}/links/remove`)) {
					linkMutations += 1;
				}
			});
			const refused = `Invented weekly ${action} note withheld by another writer.`;
			await loser.getByLabel('Weekly overview.').fill(refused);
			await losing.arrived;
			await page.goto(seeded.summaryUrl);
			const winning = `Invented winning weekly ${action} note.`;
			await page.getByLabel('Weekly overview.').fill(winning);
			await expectSaved(page);
			await expectUnloadReady(page);
			await loser.getByRole('button', { name: action === 'add' ? 'Add link' : 'Remove' }).click();
			const refusedSave = loser.waitForResponse((response) =>
				response.request().method() === 'PUT' &&
				new URL(response.url()).pathname === `/api/drafts/${seeded.summaryId}/content` &&
				response.status() === 409
			);
			losing.release();
			await refusedSave;
			await reload.arrived;
			const later = `Invented weekly ${action} follow-up typed during the winner reload.`;
			await loser.getByLabel('Weekly follow-up.').fill(later);
			await expect(loser.getByRole('alert')).toBeVisible();
			expect({ linkMutations, reloads }, 'refusal must stop link mutation and a second reload')
				.toEqual({ linkMutations: 0, reloads: 1 });
			await expect(loser.getByRole('alert')).toContainText('Your refused text is still here');
			reload.release();
			await expect(refusedText(loser, 'Weekly overview.')).toHaveText(refused);
			await expect(refusedText(loser, 'Weekly follow-up.')).toHaveText(later);
			await expect(loser.getByLabel('Weekly overview.')).toHaveValue(winning);
			const summary = await (await page.request.get(`/api/drafts/${seeded.summaryId}`)).json();
			expect(summary.summary_links).toHaveLength(action === 'add' ? 0 : 1);
		} finally {
			losing.release();
			reload.release();
			await loserContext.close();
		}
	});
}

test('a late weekly link response cannot change the next draft editor', async ({
	page,
	setupCode,
	browser
}) => {
	const seeded = await seedWeekly(page, browser, setupCode);
	const session = await secondSession(page, seeded.jordanUserId);
	const other = await page.request.post(
		`/api/sessions/${session.sessionId}/draft`, { data: {} }
	);
	if (!other.ok()) throw new Error(`second daily draft failed: ${other.status()}`);
	const otherId: number = (await other.json()).id;
	const authorContext = seeded.coordinatorContext;
	const author = seeded.coordinator;
	const linked = gate();
	try {
		await author.goto(seeded.summaryUrl);
		await markSpa(author);
		await author.getByLabel('Link a daily report').selectOption(String(seeded.dailyVersionId));
		await author.route(`**/api/drafts/${seeded.summaryId}/links`, async (route) => {
			if (route.request().method() !== 'POST') {
				await route.continue();
				return;
			}
			const response = await route.fetch();
			if (!response.ok()) throw new Error(`source link failed: ${response.status()}`);
			linked.arrive();
			await linked.released;
			await route.fulfill({ response });
		});
		await author.getByRole('button', { name: 'Add link' }).click();
		await linked.arrived;
		const sourcePaused = await author.getByLabel('Weekly overview.').isDisabled();
		await spaToDraft(author, otherId, session.traineeName);
		const destinationBefore = await (await page.request.get(`/api/drafts/${otherId}`)).json();
		const answered = author.waitForResponse((response) =>
			response.request().method() === 'POST' &&
			new URL(response.url()).pathname === `/api/drafts/${seeded.summaryId}/links`
		);
		linked.release();
		await answered;
		await author.waitForLoadState('networkidle');
		const fresh = 'Invented note on the later daily draft.';
		const destinationSave = author.waitForRequest((request) =>
			request.method() === 'PUT' &&
			new URL(request.url()).pathname === `/api/drafts/${otherId}/content`
		);
		await author.getByLabel(LEAST).fill(fresh);
		const submitted = await destinationSave;
		expect(submitted.postDataJSON().revision, 'late source response changed the destination revision')
			.toBe(destinationBefore.revision);
		await expectSaved(author);
		expect(sourcePaused, 'the source editor accepted typing during its link request').toBe(true);
		await expect(author.locator('details.refused')).toHaveCount(0);
		const destination = await (await page.request.get(`/api/drafts/${otherId}`)).json();
		expect(destination.content.narratives.some((entry: { text: string }) => entry.text === fresh))
			.toBe(true);
	} finally {
		linked.release();
		await authorContext.close();
	}
});
