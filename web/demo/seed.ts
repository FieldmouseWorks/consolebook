// Seeds a fresh installation with an invented training center: a published
// program, six people in four roles, and two trainees' worth of sessions,
// daily reports, a weekly summary, an amendment, acknowledgments, and task
// signoffs. Everything goes through the public HTTP API as the person who
// would do it, so the demo can hold nothing the product would refuse. Every
// agency, person, call, and phone number here is invented.

export const DEMO_PASSWORD = 'consolebook-demo';
export const AGENCY = 'Example County Emergency Communications';
const TIMEZONE = 'America/Los_Angeles';

export const PEOPLE = {
	admin: { username: 'avery.morgan', display_name: 'Avery Morgan', role: 'administrator' },
	coordinator: { username: 'casey.lin', display_name: 'Casey Lin', role: 'coordinator' },
	jordan: { username: 'jordan.reyes', display_name: 'Jordan Reyes', role: 'trainer' },
	rowan: { username: 'rowan.patel', display_name: 'Rowan Patel', role: 'trainer' },
	taylor: { username: 'taylor.brooks', display_name: 'Taylor Brooks', role: 'trainee' },
	riley: { username: 'riley.chen', display_name: 'Riley Chen', role: 'trainee' }
} as const;

type Person = keyof typeof PEOPLE;

const STANDARD = 'Standard 1-7';
const COMPETENCIES = [
	{
		category: 'Call processing',
		name: 'Call Interrogation',
		description: 'Obtains and verifies location, callback number, and nature of the call.',
		tasks: [
			'Verifies the location and callback number on every call.',
			'Follows the medical interrogation protocol to a determinant.'
		]
	},
	{
		category: 'Call processing',
		name: 'Telephone Demeanor',
		description: 'Calm, controlled, and courteous with every caller.',
		tasks: ['Takes control of a hysterical caller and gets usable information.']
	},
	{
		category: 'Call processing',
		name: 'CAD Entry',
		description: 'Enters incidents accurately with the right type code and priority.',
		tasks: ['Enters an incident with the correct type code and priority.']
	},
	{
		category: 'Radio',
		name: 'Radio Discipline',
		description: 'Plain language, correct unit identifiers, no wasted airtime.',
		tasks: ['Dispatches a priority call using plain language and correct identifiers.']
	},
	{
		category: 'Radio',
		name: 'Unit Status Tracking',
		description: 'Knows where every unit is and what it is doing.',
		tasks: ['Tracks unit status through a multi-unit incident without prompting.']
	},
	{
		category: 'Policy and judgment',
		name: 'Knowledge of Procedures',
		description: 'Finds and applies the right standard operating procedure.',
		tasks: ['Locates the correct procedure without help.']
	},
	{
		category: 'Policy and judgment',
		name: 'Problem Solving',
		description: 'Makes sound decisions when the call does not fit the script.',
		tasks: []
	}
];

const PHASES = [
	{ name: 'Phase 1: Call receiving', description: 'Answering 911 and non-emergency lines.' },
	{ name: 'Phase 2: Law dispatch', description: 'Working a law enforcement radio channel.' },
	{ name: 'Phase 3: Fire and EMS dispatch', description: 'Working the fire and EMS channel.' },
	{ name: 'Phase 4: Shadow', description: 'Working solo with a trainer observing.' }
];

const PROGRAM = {
	name: 'Communications Training Officer Program',
	label: '2026 rev A',
	description:
		'Daily observation reports on a 1-7 scale, weekly summaries, and task signoffs across four phases.',
	phases: PHASES.map((phase, index) => ({ ...phase, presentation_number: index + 1 })),
	phase_transitions: [
		{ from_phase: PHASES[0].name, to_phase: PHASES[1].name, kind: 'advance' },
		{ from_phase: PHASES[1].name, to_phase: PHASES[2].name, kind: 'advance' },
		{ from_phase: PHASES[2].name, to_phase: PHASES[3].name, kind: 'advance' },
		{ from_phase: PHASES[1].name, to_phase: PHASES[0].name, kind: 'remediation' },
		{ from_phase: PHASES[2].name, to_phase: PHASES[1].name, kind: 'remediation' }
	],
	competencies: COMPETENCIES.map((competency) => ({
		category: competency.category,
		name: competency.name,
		description: competency.description,
		tasks: competency.tasks.map((prompt) => ({ prompt, citations: [] })),
		citations: []
	})),
	rating_scales: [
		{
			name: STANDARD,
			kind: 'anchored_numeric',
			min_value: 1,
			max_value: 7,
			anchors: [
				{ value: 1, label: 'Unacceptable', definition: 'Not acceptable by the agency standard.' },
				{ value: 4, label: 'Acceptable', definition: 'Meets the agency standard for this phase.' },
				{ value: 7, label: 'Superior', definition: 'Well above the agency standard.' }
			]
		}
	],
	rating_modifiers: [
		{
			code: 'NRT',
			label: 'Not responding to training',
			description: 'The same deficiency after documented remedial training.'
		},
		{
			code: 'RMD',
			label: 'Remedial training given',
			description: 'Remedial training was given today; the narrative says what.'
		}
	],
	evaluation_forms: [
		{
			record_type: 'daily_report',
			name: 'Daily Observation Report',
			instructions: 'Rate what you observed this shift. Mark a competency not observed rather than guess.',
			competencies: COMPETENCIES.map((competency) => ({
				competency: competency.name,
				rating_scale: STANDARD
			})),
			narratives: [
				{ prompt: 'Most acceptable performance', required: true },
				{ prompt: 'Least acceptable performance', required: true },
				{ prompt: 'Additional comments', required: false }
			]
		},
		{
			record_type: 'weekly_summary',
			name: 'Weekly Summary',
			instructions: 'Summarize the week from the daily reports it covers.',
			competencies: [],
			narratives: [
				{ prompt: 'Progress this week', required: true },
				{ prompt: 'Goals for next week', required: false }
			]
		}
	],
	citations: [],
	finalization_policy: {
		review_approved: true,
		required_narratives: true,
		ratings_complete: true
	}
};

/** One shift's daily report: a rating per competency (null = not observed). */
interface Shift {
	daysAgo: number;
	ratings: (number | null)[];
	most: string;
	least: string;
	comments?: string;
	remedial?: string;
}

const TAYLOR_SHIFTS: Shift[] = [
	{
		daysAgo: 16,
		ratings: [3, 4, 3, null, null, 3, 4],
		most: 'Answered every 911 line within ten seconds. Polite and steady with a confused elderly caller reporting a fall (invented call, 555-0142).',
		least: 'Twice entered the incident before verifying the callback number. Discussed why the callback comes first.',
		remedial: 'Reviewed the location-first interrogation sequence for 20 minutes.'
	},
	{
		daysAgo: 15,
		ratings: [4, 4, 3, null, null, 3, 4],
		most: 'Verified location and callback on all calls observed. Good use of the medical protocol on an invented chest-pain call.',
		least: 'Picked the wrong type code for a non-injury collision; caught it on review.'
	},
	{
		daysAgo: 14,
		ratings: [4, 5, 4, null, null, 4, 4],
		most: 'Kept control of a hysterical caller on an invented structure fire on Alder Lane and got a usable address in under 40 seconds.',
		least: 'Hesitated finding the hazmat procedure. Showed where it lives in the binder and in CAD.'
	},
	{
		daysAgo: 13,
		ratings: [5, 5, 4, null, null, 4, 5],
		most: 'Smooth shift. Handled a busy hour of invented weather calls without falling behind.',
		least: 'Nothing significant. Minor typo in one incident narrative.',
		comments: 'Ready to start on law dispatch next set.'
	},
	{
		daysAgo: 9,
		ratings: [5, 5, 4, 3, 3, 4, 4],
		most: 'First day on the law channel. Good instincts on which calls to hold and which to send now.',
		least: 'Lost track of two units on a traffic stop. Talked through the status board habits.',
		remedial: 'Status board drill: 30 minutes on the training console.'
	},
	{
		daysAgo: 8,
		ratings: [5, 6, 5, 4, 3, 4, 5],
		most: 'Clear, short transmissions on an invented pursuit; officers had what they needed.',
		least: 'Still checking unit status late during busy stretches.'
	},
	{
		daysAgo: 7,
		ratings: [5, 5, 5, 4, 4, 5, 5],
		most: 'Ran the channel solo for the last two hours with no prompts.',
		least: 'Used a ten-code once out of habit; corrected immediately.'
	},
	{
		daysAgo: 6,
		ratings: [6, 6, 5, 5, 4, 5, 5],
		most: 'Handled an invented domestic disturbance with three units cleanly, from dispatch to clear.',
		least: 'Nothing significant this shift.'
	}
];

const RILEY_SHIFTS: Shift[] = [
	{
		daysAgo: 4,
		ratings: [3, 4, 3, null, null, 3, 3],
		most: 'Friendly and patient on the non-emergency line. Learned the CAD layout quickly.',
		least: 'Needs to slow down on the address readback; one invented address was transposed.'
	},
	{
		daysAgo: 3,
		ratings: [4, 4, 3, null, null, 3, 4],
		most: 'Address readback fixed. Took an invented welfare check call start to finish.',
		least: 'Unsure which calls go to the law queue and which stay on the phone.'
	}
];

/** A minimal cookie-carrying client for one signed-in person. */
class Client {
	private cookie = '';
	private readonly base: string;

	constructor(base: string) {
		this.base = base;
	}

	async call<T>(method: string, path: string, body?: unknown): Promise<T> {
		const headers: Record<string, string> = {};
		if (body !== undefined) headers['Content-Type'] = 'application/json';
		if (this.cookie) headers.Cookie = this.cookie;
		const response = await fetch(`${this.base}${path}`, {
			method,
			headers,
			body: body === undefined ? undefined : JSON.stringify(body)
		});
		const session = response.headers
			.getSetCookie()
			.find((cookie) => cookie.startsWith('consolebook_session='));
		if (session) this.cookie = session.split(';')[0];
		if (!response.ok) {
			throw new Error(`${method} ${path} returned ${response.status}: ${await response.text()}`);
		}
		return response.status === 204 ? (undefined as T) : ((await response.json()) as T);
	}

	get<T>(path: string) {
		return this.call<T>('GET', path);
	}

	post<T>(path: string, body: unknown = {}) {
		return this.call<T>('POST', path, body);
	}

	put<T>(path: string, body: unknown) {
		return this.call<T>('PUT', path, body);
	}
}

interface RatingEntry {
	form_competency_id: number;
	value: number | null;
	not_observed: boolean;
	modifier_ids: number[];
}

interface DraftView {
	revision: number;
	content: {
		ratings: RatingEntry[];
		narratives: { form_narrative_id: number; text: string }[];
	};
	form: {
		competencies: { form_competency_id: number }[];
		narratives: { form_narrative_id: number }[];
		modifiers: { rating_modifier_id: number; code: string }[];
	};
}

/** The agency-local calendar date `daysAgo` days before today. */
function businessDate(daysAgo: number): string {
	const day = new Date(Date.now() - daysAgo * 86_400_000);
	return new Intl.DateTimeFormat('en-CA', { timeZone: TIMEZONE }).format(day);
}

/** A backdated instant for phase history: the morning of `daysAgo`. */
function secondsAgo(daysAgo: number): number {
	return Math.floor(Date.now() / 1000) - daysAgo * 86_400;
}

export interface SeededDemo {
	taylorEnrollment: number;
	rileyEnrollment: number;
	/** Taylor's amended daily report: version 2 of a sealed record. */
	amendedRecord: number;
	/** The daily report Jordan is still writing. */
	openDraft: number;
	weeklySummary: number;
}

export async function seedDemo(base: string, setupCode: string): Promise<SeededDemo> {
	const anonymous = new Client(base);
	await anonymous.post('/api/setup', {
		setup_code: setupCode,
		agency_name: AGENCY,
		username: PEOPLE.admin.username,
		display_name: PEOPLE.admin.display_name,
		password: DEMO_PASSWORD
	});

	const as: Partial<Record<Person, Client>> = {};
	const ids: Partial<Record<Person, number>> = {};
	async function signIn(person: Person): Promise<Client> {
		const client = new Client(base);
		const session = await client.post<{ user: { id: number } }>('/api/auth/login', {
			username: PEOPLE[person].username,
			password: DEMO_PASSWORD
		});
		ids[person] = session.user.id;
		as[person] = client;
		return client;
	}

	const admin = await signIn('admin');
	for (const person of ['coordinator', 'jordan', 'rowan', 'taylor', 'riley'] as const) {
		const { username, display_name, role } = PEOPLE[person];
		const created = await admin.post<{ id: number; reset_code: string }>('/api/users', {
			username,
			display_name,
			role
		});
		await new Client(base).post('/api/auth/reset', {
			username,
			reset_code: created.reset_code,
			new_password: DEMO_PASSWORD
		});
		await signIn(person);
	}
	const casey = as.coordinator!;

	const program = await admin.post<{ id: number }>('/api/programs', { name: PROGRAM.name });
	const version = await admin.post<{ id: number }>(`/api/programs/${program.id}/versions`, PROGRAM);
	await admin.post(`/api/program-versions/${version.id}/publish`);

	async function enroll(trainee: Person, trainer: Person, startDaysAgo: number) {
		const enrollment = await admin.post<{ id: number }>(
			`/api/program-versions/${version.id}/enrollments`,
			{ user_id: ids[trainee] }
		);
		await casey.post(`/api/enrollments/${enrollment.id}/assignments`, {
			trainer_user_id: ids[trainer]
		});
		const detail = await casey.get<{ phases: { id: number; name: string }[] }>(
			`/api/enrollments/${enrollment.id}`
		);
		await casey.post(`/api/enrollments/${enrollment.id}/phase-events`, {
			kind: 'advance',
			to_phase_id: detail.phases[0].id,
			effective_at: secondsAgo(startDaysAgo),
			reason: ''
		});
		return { id: enrollment.id, phases: detail.phases };
	}

	/** Records the shift, then the trainer drafts its daily report. */
	async function shift(enrollmentId: number, trainer: Person, entry: Shift, phaseId: number) {
		const date = businessDate(entry.daysAgo);
		const session = await casey.post<{ id: number }>(`/api/enrollments/${enrollmentId}/sessions`, {
			business_date: date,
			timezone: TIMEZONE,
			local_start: `${date}T07:00`,
			local_end: `${date}T17:00`,
			disposition: 'completed',
			phase_id: phaseId,
			trainer_user_ids: [ids[trainer]]
		});
		const author = as[trainer]!;
		const draft = await author.post<{ id: number }>(`/api/sessions/${session.id}/draft`);
		const view = await author.get<DraftView>(`/api/drafts/${draft.id}`);
		const remedial = view.form.modifiers.find((modifier) => modifier.code === 'RMD');
		const narrativeTexts = [entry.most, entry.least, [entry.remedial, entry.comments].filter(Boolean).join(' ')];
		const saved = await author.put<{ revision: number }>(`/api/drafts/${draft.id}/content`, {
			revision: view.revision,
			ratings: view.form.competencies.map((competency, index) => ({
				form_competency_id: competency.form_competency_id,
				value: entry.ratings[index],
				not_observed: entry.ratings[index] === null,
				modifier_ids:
					entry.remedial && remedial && index === 0 ? [remedial.rating_modifier_id] : []
			})),
			narratives: view.form.narratives
				.map((narrative, index) => ({
					form_narrative_id: narrative.form_narrative_id,
					text: narrativeTexts[index] ?? ''
				}))
				.filter((narrative) => narrative.text !== '')
		});
		return { draftId: draft.id, revision: saved.revision };
	}

	/** Submit, approve, and seal: the ordinary path to a finalized record. */
	async function seal(trainer: Person, draftId: number, revision: number) {
		await as[trainer]!.post(`/api/drafts/${draftId}/submit`, { revision });
		await casey.post(`/api/drafts/${draftId}/review`, { decision: 'approved' });
		const reviewed = await casey.get<DraftView>(`/api/drafts/${draftId}`);
		await casey.post(`/api/drafts/${draftId}/finalize`, { revision: reviewed.revision });
	}

	// Taylor: four shifts in Phase 1, then law dispatch.
	const taylor = await enroll('taylor', 'jordan', TAYLOR_SHIFTS[0].daysAgo);
	const taylorDailies: number[] = [];
	for (const [index, entry] of TAYLOR_SHIFTS.entries()) {
		if (index === 4) {
			await casey.post(`/api/enrollments/${taylor.id}/phase-events`, {
				kind: 'advance',
				to_phase_id: taylor.phases[1].id,
				effective_at: secondsAgo(entry.daysAgo),
				reason: 'Ratings at or above standard for a full set; ready for law dispatch.'
			});
		}
		const phase = taylor.phases[index < 4 ? 0 : 1];
		const { draftId, revision } = await shift(taylor.id, 'jordan', entry, phase.id);
		await seal('jordan', draftId, revision);
		taylorDailies.push(draftId);
	}

	// Taylor acknowledges all but the two newest; one with a response.
	const taylorClient = as.taylor!;
	for (const [index, recordId] of taylorDailies.slice(0, -2).entries()) {
		await taylorClient.post(`/api/drafts/${recordId}/acknowledge`, {
			kind: index === 2 ? 'acknowledged_with_response' : 'acknowledged',
			response:
				index === 2
					? 'Understood on the hazmat procedure. I have bookmarked it in CAD.'
					: undefined
		});
	}

	// A correction is an amendment: the second report had the wrong CAD
	// rating, so it is reopened, fixed, and sealed again as version 2.
	const amendedRecord = taylorDailies[1];
	await casey.post(`/api/drafts/${amendedRecord}/amend`, {
		reason: 'CAD Entry was rated 3; the trainer meant 4. The type-code slip was caught on review, not live.'
	});
	const jordan = as.jordan!;
	const reopened = await jordan.get<DraftView>(`/api/drafts/${amendedRecord}`);
	const corrected = await jordan.put<{ revision: number }>(`/api/drafts/${amendedRecord}/content`, {
		revision: reopened.revision,
		ratings: reopened.content.ratings.map((rating, index) =>
			index === 2 ? { ...rating, value: 4 } : rating
		),
		narratives: reopened.content.narratives
	});
	await seal('jordan', amendedRecord, corrected.revision);

	// Jordan's weekly summary covers the Phase 1 week; Casey approves it.
	const summary = await jordan.post<{ id: number }>(`/api/enrollments/${taylor.id}/weekly-summary`);
	const linkable = await jordan.get<{ dailies: { daily_version_id: number; record_id: number }[] }>(
		`/api/drafts/${summary.id}/linkable-dailies`
	);
	for (const daily of linkable.dailies.filter((d) => taylorDailies.slice(0, 4).includes(d.record_id))) {
		const current = await jordan.get<DraftView>(`/api/drafts/${summary.id}`);
		await jordan.post(`/api/drafts/${summary.id}/links`, {
			daily_version_id: daily.daily_version_id,
			revision: current.revision
		});
	}
	const summaryView = await jordan.get<DraftView>(`/api/drafts/${summary.id}`);
	const summarySaved = await jordan.put<{ revision: number }>(`/api/drafts/${summary.id}/content`, {
		revision: summaryView.revision,
		ratings: [],
		narratives: [
			{
				form_narrative_id: summaryView.form.narratives[0].form_narrative_id,
				text: 'Steady improvement across the set. Location and callback verification is now habit. CAD type codes still need attention on unusual calls.'
			},
			{
				form_narrative_id: summaryView.form.narratives[1].form_narrative_id,
				text: 'Start on the law channel. Build the status board habit early.'
			}
		]
	});
	await jordan.post(`/api/drafts/${summary.id}/submit`, { revision: summarySaved.revision });
	await casey.post(`/api/drafts/${summary.id}/review`, { decision: 'approved' });
	const approvedSummary = await jordan.get<DraftView>(`/api/drafts/${summary.id}`);
	await casey.post(`/api/drafts/${summary.id}/finalize`, { revision: approvedSummary.revision });
	await taylorClient.post(`/api/drafts/${summary.id}/acknowledge`, { kind: 'acknowledged' });

	// Signoffs for what Taylor has shown so far.
	const matrix = await casey.get<{ tasks: { task_id: number }[] }>(
		`/api/enrollments/${taylor.id}/signoffs`
	);
	for (const [index, task] of matrix.tasks.slice(0, 5).entries()) {
		await casey.post(`/api/enrollments/${taylor.id}/signoffs`, {
			task_id: task.task_id,
			kind: index < 3 ? 'demonstrated' : 'observed'
		});
	}

	// Yesterday's shift: Jordan has started the report but not submitted it.
	const yesterday = await shift(
		taylor.id,
		'jordan',
		{
			daysAgo: 1,
			ratings: [6, 6, 5, 5, 5, 5, 6],
			most: 'Worked the law channel through a busy afternoon with no prompts. Clean handoffs at shift change.',
			least: ''
		},
		taylor.phases[1].id
	);

	// Riley: just started, with a report waiting in the review queue.
	const riley = await enroll('riley', 'rowan', RILEY_SHIFTS[0].daysAgo);
	for (const entry of RILEY_SHIFTS) {
		const { draftId, revision } = await shift(riley.id, 'rowan', entry, riley.phases[0].id);
		if (entry === RILEY_SHIFTS[0]) await seal('rowan', draftId, revision);
		else await as.rowan!.post(`/api/drafts/${draftId}/submit`, { revision });
	}

	return {
		taylorEnrollment: taylor.id,
		rileyEnrollment: riley.id,
		amendedRecord,
		openDraft: yesterday.draftId,
		weeklySummary: summary.id
	};
}
