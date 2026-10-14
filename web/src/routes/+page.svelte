<script lang="ts">
	import { goto, invalidateAll } from '$app/navigation';
	import {
		ApiError,
		closeSession,
		createDraft,
		createUser,
		dailyForms,
		downloadExport,
		exportSummary,
		getHealth,
		getNotices,
		getSetupProgress,
		installationExportPath,
		issueResetCode,
		markNoticeRead,
		myAssignments,
		mySessions,
		reviewQueue,
		type AssignedTrainee,
		type CreatedUser,
		type ExportSummary,
		type Health,
		type MySession,
		type Notice,
		type ReviewQueueRow,
		type SetupProgress,
		type Role,
		type SessionDisposition
	} from '$lib/api';
	import { capabilityName, instant, localSpan } from '$lib/format';
	import type { ShellData } from './+layout';

	let { data }: { data: ShellData } = $props();

	// The layout guard redirects unauthenticated visitors, so session is
	// present here; the fallback satisfies the type system honestly.
	let session = $derived(data.session);
	let canManageUsers = $derived(
		data.session?.capabilities.includes('manage_users') ?? false
	);
	let canViewAssigned = $derived(
		data.session?.capabilities.includes('view_assigned_records') ?? false
	);
	let canAuthor = $derived(
		data.session?.capabilities.includes('author_evaluation') ?? false
	);
	let canReview = $derived(
		data.session?.capabilities.includes('review_evaluation') ?? false
	);
	let canViewOwn = $derived(
		data.session?.capabilities.includes('view_own_records') ?? false
	);
	let canExport = $derived(
		data.session?.capabilities.includes('export_records') ?? false
	);

	let health: Health | null = $state(null);
	let notices: Notice[] = $state([]);
	let assigned: AssignedTrainee[] = $state([]);
	let sessions: MySession[] = $state([]);
	let queue: ReviewQueueRow[] = $state([]);
	let summary: ExportSummary | null = $state(null);
	let progress: SetupProgress | null = $state(null);
	$effect(() => {
		if (canManageUsers) {
			getSetupProgress().then(
				(body) => (progress = body),
				() => (progress = null)
			);
		}
		getHealth().then(
			(h) => (health = h),
			() => (health = null)
		);
		getNotices().then(
			(body) => (notices = body.notices),
			() => (notices = [])
		);
		if (canViewAssigned) {
			myAssignments().then(
				(body) => (assigned = body.assignments),
				() => (assigned = [])
			);
		}
		if (canAuthor) {
			mySessions().then(
				(body) => (sessions = body.sessions),
				() => (sessions = [])
			);
		}
		if (canReview) {
			reviewQueue().then(
				(body) => (queue = body.drafts),
				() => (queue = [])
			);
		}
		if (canExport) {
			exportSummary().then(
				(body) => (summary = body),
				() => (summary = null)
			);
		}
	});

	async function acknowledge(notice: Notice) {
		await markNoticeRead(notice.id);
		notices = (await getNotices()).notices;
		await invalidateAll();
	}

	// Unread notices lead; read ones stay one click away. Five unread are
	// shown before the rest fold behind "Show all".
	const NOTICE_PREVIEW = 5;
	let showAllNotices = $state(false);
	let unread = $derived(notices.filter((n) => n.read_at === null));
	let shownNotices = $derived(
		showAllNotices ? notices : unread.slice(0, NOTICE_PREVIEW)
	);
	let markingAll = $state(false);
	async function markAllRead() {
		markingAll = true;
		try {
			for (const notice of unread) {
				await markNoticeRead(notice.id);
			}
		} finally {
			notices = (await getNotices()).notices;
			markingAll = false;
			await invalidateAll();
		}
	}

	let resetUsername = $state('');
	let issued: { username: string; reset_code: string; expires_at: number } | null =
		$state(null);
	let issueError = $state('');
	let busy = $state(false);

	// Session members work their sessions from here even without a durable
	// assignment: membership is the grant.
	let sessionEnd: Record<number, string> = $state({});
	let sessionError = $state('');
	async function closeMine(session: MySession, disposition: SessionDisposition) {
		sessionError = '';
		busy = true;
		try {
			await closeSession(
				session.session_id,
				disposition,
				disposition === 'cancelled' ? undefined : sessionEnd[session.session_id]
			);
			sessions = (await mySessions()).sessions;
		} catch (err) {
			sessionError =
				err instanceof ApiError ? err.message : 'the server could not be reached';
		} finally {
			busy = false;
		}
	}

	// A version may pin several daily forms; the picker appears exactly
	// when the choice is real.
	let draftForms: Record<number, { id: number; name: string }[]> = $state({});
	let draftFormChoice: Record<number, number> = $state({});

	async function startDraft(session: MySession, formId?: number) {
		sessionError = '';
		busy = true;
		try {
			if (formId === undefined) {
				const { forms } = await dailyForms(session.session_id);
				if (forms.length > 1) {
					draftForms[session.session_id] = forms;
					draftFormChoice[session.session_id] = forms[0].id;
					return;
				}
				formId = forms[0]?.id;
			}
			const created = await createDraft(session.session_id, formId);
			await goto(`/drafts/${created.id}`);
		} catch (err) {
			sessionError =
				err instanceof ApiError ? err.message : 'the server could not be reached';
		} finally {
			busy = false;
		}
	}

	async function issueCode(event: SubmitEvent) {
		event.preventDefault();
		issueError = '';
		issued = null;
		busy = true;
		try {
			issued = await issueResetCode(resetUsername.trim());
		} catch (err) {
			issueError =
				err instanceof ApiError ? err.message : 'the server could not be reached';
		} finally {
			busy = false;
		}
	}

	// The first day, in order: each step is done when the installation's
	// counts say so, and links to where it happens (docs/install.md).
	let setupSteps = $derived.by(() => {
		const counts = progress;
		if (counts === null) return [];
		return [
			{
				label: 'Add people: a coordinator, a trainer, and a trainee',
				done:
					counts.coordinators > 0 && counts.trainers > 0 && counts.trainees > 0,
				href: '#add-person',
				hint: 'Each gets a one-time code to choose a password.'
			},
			{
				label: 'Create a training program',
				done: counts.programs > 0,
				href: '/programs',
				hint: 'Phases, competencies and tasks, a rating scale, and a form.'
			},
			{
				label: 'Publish a program version',
				done: counts.published_versions > 0,
				href: '/programs',
				hint: 'Published versions are frozen; records pin them.'
			},
			{
				label: 'Enroll a trainee',
				done: counts.enrollments > 0,
				href: '/programs',
				hint: "From the published version's page."
			},
			{
				label: 'Assign a trainer',
				done: counts.assignments > 0,
				href: null,
				hint: "On the trainee's enrollment page."
			},
			{
				label: 'Record a training session',
				done: counts.sessions > 0,
				href: null,
				hint: "Also on the enrollment page; the trainer then sees it on their home."
			},
			{
				label: 'Finalize a first evaluation',
				done: counts.finalized_records > 0,
				href: null,
				hint: 'The trainer writes it, a coordinator reviews and finalizes it.'
			}
		];
	});
	let setupDone = $derived(setupSteps.filter((step) => step.done).length);

	let newUsername = $state('');
	let newDisplayName = $state('');
	let newEmployeeId = $state('');
	let newTitle = $state('');
	let newRole: Role = $state('trainee');
	let created: CreatedUser | null = $state(null);
	let createError = $state('');

	async function addUser(event: SubmitEvent) {
		event.preventDefault();
		createError = '';
		created = null;
		busy = true;
		try {
			created = await createUser({
				username: newUsername,
				display_name: newDisplayName,
				employee_id: newEmployeeId,
				title: newTitle,
				role: newRole
			});
			newUsername = '';
			newDisplayName = '';
			newEmployeeId = '';
			newTitle = '';
			newRole = 'trainee';
			progress = await getSetupProgress().catch(() => progress);
		} catch (err) {
			createError =
				err instanceof ApiError ? err.message : 'the server could not be reached';
		} finally {
			busy = false;
		}
	}

	// The whole installation leaves as one archive: every finalized
	// version as its stored bytes, verifiable from the archive alone
	// (ADR 0014). This is the explicit export_records authority.
	let exportError = $state('');
	let exported = $state('');
	async function exportInstallation() {
		exportError = '';
		exported = '';
		busy = true;
		try {
			exported = await downloadExport(installationExportPath());
		} catch (err) {
			exportError =
				err instanceof ApiError ? err.message : 'the server could not be reached';
		} finally {
			busy = false;
		}
	}
</script>

<h1>Home</h1>
<p class="lede">Signed in as {session?.user.display_name} · {data.instance.agency}</p>

{#if setupSteps.length > 0 && setupDone < setupSteps.length}
	<section class="panel setup">
		<div class="cardhead">
			<h2>Getting started</h2>
			<span class="count">{setupDone} of {setupSteps.length} steps done</span>
		</div>
		<ol class="steps">
			{#each setupSteps as step (step.label)}
				<li class:done={step.done}>
					<span class="mark" aria-hidden="true">{step.done ? '✓' : ''}</span>
					<span>
						{#if step.href && !step.done}
							<a href={step.href}>{step.label}</a>
						{:else}
							{step.label}
						{/if}
						{#if step.done}<span class="visually-hidden">(done)</span>{/if}
						<span class="sub">{step.hint}</span>
					</span>
				</li>
			{/each}
		</ol>
	</section>
{/if}

{#if canReview}
	<section class="card">
		<h2>Review queue</h2>
		{#if queue.length === 0}
			<p class="quiet">Nothing awaiting review.</p>
		{:else}
			<table class="grid">
				<thead>
					<tr>
						<th>Trainee</th>
						<th>Program</th>
						<th>Owner</th>
						<th>Submitted</th>
						<th></th>
					</tr>
				</thead>
				<tbody>
					{#each queue as row (row.record_id)}
						<tr>
							<td>{row.trainee_display_name}</td>
							<td>{row.program_name} — v{row.version_number}</td>
							<td>{row.owner_display_name}</td>
							<td class="nowrap">{instant(row.submitted_at)}</td>
							<td>
								<a href={`/drafts/${row.record_id}`}>
									{row.eligible ? 'Review' : 'Open'}
								</a>
								{#if !row.eligible}
									<span class="quiet-inline">(you contributed)</span>
								{/if}
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		{/if}
	</section>
{/if}

{#if canAuthor}
	<section class="panel">
		<h2>My sessions</h2>
		{#if sessions.length === 0}
			<p class="quiet">No sessions yet. A coordinator records sessions on a trainee's enrollment.</p>
		{:else}
			<table class="grid sessions">
				<thead>
					<tr>
						<th>When</th>
						<th>Trainee</th>
						<th>Status</th>
						<th>Daily report</th>
						<th>Close session</th>
					</tr>
				</thead>
				<tbody>
					{#each sessions as session (session.session_id)}
						{@const span = localSpan(session.local_start, session.local_end)}
						<tr>
							<td class="nowrap when">
								<strong>{session.business_date}</strong>
								<span class="sub">{span.times} <abbr title={session.timezone}>local</abbr></span>
							</td>
							<td>
								<a href={`/enrollments/${session.enrollment_id}`}>{session.trainee_display_name}</a>
								<span class="sub">{session.program_name} — v{session.version_number}</span>
							</td>
							<td class="state">
								<span class="pill status {session.disposition ?? 'open'}">
									{session.disposition ?? 'open'}
								</span>
							</td>
							<td>
								{#if session.draft_id !== null}
									<a href={`/drafts/${session.draft_id}`}>Open draft</a>
								{:else if session.disposition !== 'cancelled'}
									{#if draftForms[session.session_id]}
										<span class="sessionbar">
											<select
												aria-label="Daily form"
												bind:value={draftFormChoice[session.session_id]}
											>
												{#each draftForms[session.session_id] as form (form.id)}
													<option value={form.id}>{form.name}</option>
												{/each}
											</select>
											<button
												type="button"
												class="secondary small"
												disabled={busy}
												onclick={() =>
													startDraft(session, draftFormChoice[session.session_id])}
											>
												Create
											</button>
										</span>
									{:else}
										<button
											type="button"
											class="secondary small"
											disabled={busy}
											onclick={() => startDraft(session)}
										>
											Start draft
										</button>
									{/if}
								{:else}
									<span class="quiet-inline">—</span>
								{/if}
							</td>
							<td class:closed={session.disposition !== null}>
								{#if session.disposition === null}
									<div class="closebar">
										<label class="inline">
											Local end
											<input
												type="datetime-local"
												bind:value={sessionEnd[session.session_id]}
											/>
										</label>
										<div class="sessionbar">
											<button
												type="button"
												class="small"
												disabled={busy || !sessionEnd[session.session_id]}
												onclick={() => closeMine(session, 'completed')}
											>
												Complete
											</button>
											<button
												type="button"
												class="secondary small"
												disabled={busy || !sessionEnd[session.session_id]}
												onclick={() => closeMine(session, 'interrupted')}
											>
												Interrupt
											</button>
											{#if session.draft_id === null}
												<button
													type="button"
													class="secondary small"
													disabled={busy}
													onclick={() => closeMine(session, 'cancelled')}
												>
													Cancel session
												</button>
											{/if}
										</div>
									</div>
								{/if}
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		{/if}
		{#if sessionError}
			<p class="error" role="alert">{sessionError}</p>
		{/if}
	</section>
{/if}

{#if canViewAssigned}
	<section class="panel">
		<h2>My trainees</h2>
		{#if assigned.length === 0}
			<p class="quiet">No active training assignments.</p>
		{:else}
			<table class="grid">
				<thead>
					<tr>
						<th>Trainee</th>
						<th>Program</th>
						<th>Assigned</th>
					</tr>
				</thead>
				<tbody>
					{#each assigned as row (row.assignment_id)}
						<tr>
							<td>
								<a href={`/enrollments/${row.enrollment_id}`}>
									{row.trainee_display_name}
								</a>
							</td>
							<td>{row.program_name} — v{row.version_number}</td>
							<td class="nowrap">{instant(row.assigned_at)}</td>
						</tr>
					{/each}
				</tbody>
			</table>
		{/if}
	</section>
{/if}

{#if canViewOwn && !canAuthor && !canReview && !canManageUsers}
	<section class="card">
		<h2>Your training records</h2>
		<p>
			Your finalized evaluations, task signoffs, and downloadable packets are
			kept together. When a trainer's evaluation of you is finalized it appears
			there for you to acknowledge, and a notice shows here.
		</p>
		<p><a href="/records">Open your training records</a></p>
	</section>
{/if}

<section class="panel">
	<div class="cardhead">
		<h2>Notices</h2>
		{#if unread.length > 0}
			<span class="count">{unread.length} unread</span>
			<button
				type="button"
				class="secondary small"
				disabled={markingAll}
				onclick={markAllRead}
			>
				Mark all read
			</button>
		{/if}
	</div>
	{#if notices.length === 0}
		<p class="quiet">No notices.</p>
	{:else}
		{#if shownNotices.length === 0}
			<p class="quiet">Nothing new.</p>
		{:else}
			<ul class="notices">
				{#each shownNotices as notice (notice.id)}
					<li class:unread={notice.read_at === null}>
						<p>{notice.message}</p>
						<p class="meta">
							{instant(notice.created_at)}
							{#if notice.read_at === null}
								<button class="secondary small" onclick={() => acknowledge(notice)}>
									Mark read
								</button>
							{/if}
						</p>
					</li>
				{/each}
			</ul>
		{/if}
		{#if notices.length > shownNotices.length || showAllNotices}
			<button
				type="button"
				class="linkish"
				aria-expanded={showAllNotices}
				onclick={() => (showAllNotices = !showAllNotices)}
			>
				{showAllNotices ? 'Show unread only' : `Show all ${notices.length} notices`}
			</button>
		{/if}
	{/if}
</section>

{#if canExport}
	<section class="card">
		<h2>Record exports</h2>
		{#if summary === null}
			<p class="quiet">Checking finalized records…</p>
		{:else if summary.version_count === 0}
			<p class="quiet">No finalized records yet; there is nothing to export.</p>
		{:else}
			<p>
				This installation holds {summary.version_count} finalized
				{summary.version_count === 1 ? 'version' : 'versions'} across
				{summary.record_count}
				{summary.record_count === 1 ? 'record' : 'records'}. An export carries
				every one as its stored canonical bytes beside a manifest and verifies
				from the archive alone with <code>consolebook-server export verify</code>.
			</p>
			<button type="button" class="secondary" disabled={busy} onclick={exportInstallation}>
				Export every finalized record
			</button>
			{#if exported}
				<p class="quiet" role="status">Downloaded {exported}.</p>
			{/if}
		{/if}
		{#if exportError}
			<p class="error" role="alert">{exportError}</p>
		{/if}
	</section>
{/if}

{#if canManageUsers}
	<section class="card" id="add-person">
		<h2>Add a person</h2>
		<p>
			The account starts with the chosen role's capabilities and no password.
			Relay the one-time code below; their first sign-in sets a password
			through the reset page.
		</p>
		<form onsubmit={addUser}>
			<label for="new-username">New username</label>
			<input id="new-username" required bind:value={newUsername} />
			<label for="new-display-name">Display name</label>
			<input id="new-display-name" bind:value={newDisplayName} />
			<label for="new-role">Role</label>
			<select id="new-role" bind:value={newRole}>
				<option value="trainee">Trainee</option>
				<option value="trainer">Trainer</option>
				<option value="coordinator">Coordinator</option>
				<option value="administrator">Administrator</option>
			</select>
			<label for="new-employee-id">Employee identifier</label>
			<input id="new-employee-id" bind:value={newEmployeeId} />
			<label for="new-title">Title</label>
			<input id="new-title" bind:value={newTitle} />
			{#if createError}
				<p class="error" role="alert">{createError}</p>
			{/if}
			{#if created}
				<p class="code-out">
					Created <strong>{created.username}</strong>. First sign-in code:
					<code>{created.reset_code}</code>
					(valid until {instant(created.reset_expires_at)})
				</p>
			{/if}
			<button type="submit" disabled={busy}>Create user</button>
		</form>
	</section>

	<section class="card">
		<h2>Issue a password reset code</h2>
		<p>
			The code is single-use, valid for 15 minutes, and shown only here — relay
			it to the user directly. Using it signs them out everywhere.
		</p>
		<form onsubmit={issueCode}>
			<label for="reset-username">Username</label>
			<input id="reset-username" required bind:value={resetUsername} />
			{#if issueError}
				<p class="error" role="alert">{issueError}</p>
			{/if}
			{#if issued}
				<p class="code-out">
					Reset code for <strong>{issued.username}</strong>:
					<code>{issued.reset_code}</code>
					(valid until {instant(issued.expires_at)})
				</p>
			{/if}
			<button type="submit" disabled={busy}>Issue reset code</button>
		</form>
	</section>
{/if}

<details class="about">
	<summary>About this installation</summary>
	<dl class="facts">
		<dt>Agency</dt>
		<dd>{data.instance.agency}</dd>
		<dt>Signed in as</dt>
		<dd>{session?.user.display_name} ({session?.user.username})</dd>
		<dt>You can</dt>
		<dd>
			{#if session && session.capabilities.length > 0}
				{session.capabilities.map(capabilityName).join(' · ')}
			{:else}
				Nothing yet; ask an administrator.
			{/if}
		</dd>
		<dt>Session expires</dt>
		<dd>{session ? instant(session.expires_at) : '—'}</dd>
		<dt>Server version</dt>
		<dd>{data.instance.version}</dd>
		<dt>Database</dt>
		<dd>{health ? health.database : 'checking…'}</dd>
	</dl>
</details>

<style>
	h2 {
		font-size: 1.1rem;
		margin: 0 0 0.5rem;
	}
	.code-out {
		background: light-dark(#eef4ee, #1e2a1f);
		border: 1px solid light-dark(#bcd4bc, #3c553d);
		border-radius: 6px;
		padding: 0.6rem 0.8rem;
		font-size: 0.92rem;
		overflow-wrap: anywhere;
	}
	code {
		font-size: 1.05em;
	}
	.quiet {
		opacity: 0.7;
		margin: 0;
	}
	ul.notices {
		list-style: none;
		margin: 0;
		padding: 0;
	}
	ul.notices li {
		border-top: 1px solid light-dark(#e2e5ea, #333a47);
		padding: 0.6rem 0;
	}
	ul.notices li:first-child {
		border-top: 0;
	}
	ul.notices li p {
		margin: 0 0 0.25rem;
	}
	ul.notices li.unread > p:first-child {
		font-weight: 600;
	}
	.meta {
		font-size: 0.82rem;
		opacity: 0.75;
		display: flex;
		align-items: center;
		gap: 0.75rem;
	}
	button.small {
		padding: 0.2rem 0.6rem;
		font-size: 0.82rem;
	}
	.sessionbar {
		display: flex;
		gap: 0.35rem;
		align-items: center;
		flex-wrap: wrap;
	}
	.sub {
		display: block;
		font-size: 0.85rem;
		opacity: 0.75;
	}
	abbr {
		text-decoration: none;
	}
	.pill.status {
		text-transform: capitalize;
	}
	.pill.status.open {
		background: light-dark(#fdf1d7, #3b301e);
		color: light-dark(#7a5410, #e4c26d);
	}
	.pill.status.completed {
		background: light-dark(#dcefdd, #1e3524);
		color: light-dark(#1e5c28, #9fd3a8);
	}
	.pill.status.interrupted,
	.pill.status.cancelled {
		background: light-dark(#eceef2, #2a303b);
		color: light-dark(#4b5361, #b4bcc8);
	}
	table.sessions td {
		vertical-align: top;
	}
	/* On a phone each session is a card: when and status on one line,
	   then the trainee, then the actions, so nothing hides off to the side. */
	@media (max-width: 48rem) {
		table.sessions {
			display: block;
			overflow: visible;
		}
		table.sessions thead {
			display: none;
		}
		table.sessions tbody,
		table.sessions tr {
			display: block;
		}
		table.sessions tr {
			display: grid;
			grid-template-columns: 1fr auto;
			gap: 0.4rem 0.75rem;
			padding: 0.75rem 0;
			border-bottom: 1px solid light-dark(#e3e6eb, #2a303b);
		}
		table.sessions td {
			display: block;
			padding: 0;
			border: 0;
			grid-column: 1 / -1;
		}
		table.sessions td.when {
			grid-column: 1;
		}
		table.sessions td.closed {
			display: none;
		}
		table.sessions td.when {
			grid-row: 1;
		}
		table.sessions td.state {
			grid-column: 2;
			grid-row: 1;
			text-align: right;
		}
		.closebar {
			min-width: 0;
		}
	}
	.closebar {
		display: grid;
		gap: 0.35rem;
		min-width: 15rem;
	}
	label.inline {
		margin: 0;
		font-size: 0.8rem;
		font-weight: 600;
		opacity: 0.85;
	}
	label.inline input {
		display: block;
		margin: 0.15rem 0 0;
		font-weight: 400;
	}
	.cardhead {
		display: flex;
		align-items: center;
		gap: 0.75rem;
		margin: 0 0 0.5rem;
	}
	.cardhead h2 {
		margin: 0;
	}
	.count {
		font-size: 0.85rem;
		opacity: 0.75;
		margin-right: auto;
	}
	button.linkish {
		background: none;
		border: 0;
		padding: 0.25rem 0;
		color: light-dark(#2456a6, #7fabf5);
		font-weight: 600;
		font-size: 0.9rem;
	}
	button.linkish:hover {
		text-decoration: underline;
	}
	ol.steps {
		list-style: none;
		margin: 0;
		padding: 0;
	}
	ol.steps li {
		display: flex;
		gap: 0.75rem;
		padding: 0.5rem 0;
		border-top: 1px solid light-dark(#e2e5ea, #333a47);
	}
	ol.steps li:first-child {
		border-top: 0;
	}
	ol.steps .mark {
		flex: none;
		width: 1.4rem;
		height: 1.4rem;
		border-radius: 999px;
		border: 2px solid light-dark(#b9c0cb, #4a5261);
		display: grid;
		place-items: center;
		font-size: 0.8rem;
		font-weight: 700;
	}
	ol.steps li.done .mark {
		background: light-dark(#2e7d4f, #4fb27a);
		border-color: transparent;
		color: #ffffff;
	}
	ol.steps li.done > span:last-child {
		opacity: 0.65;
	}
	.visually-hidden {
		position: absolute;
		width: 1px;
		height: 1px;
		overflow: hidden;
		clip-path: inset(50%);
		white-space: nowrap;
	}
	details.about {
		max-width: 44rem;
		margin: 2rem 0 0;
		font-size: 0.92rem;
	}
	details.about summary {
		cursor: pointer;
		font-weight: 600;
		opacity: 0.8;
		margin: 0 0 0.75rem;
	}
</style>
