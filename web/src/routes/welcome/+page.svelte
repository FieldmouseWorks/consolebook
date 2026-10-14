<script lang="ts">
	// Guided setup (issue #87): an optional walk from an empty installation
	// to people and a first program. It only calls the ordinary user and
	// program-import APIs, so every decision stays with the server.
	import {
		ApiError,
		createUser,
		importProgram,
		type CreatedUser,
		type ImportedBody,
		type Role
	} from '$lib/api';
	import { instant } from '$lib/format';
	import example from '$lib/setup/example-program.json';
	import type { ShellData } from '../+layout';

	let { data }: { data: ShellData } = $props();

	let canManageUsers = $derived(data.session?.capabilities.includes('manage_users') ?? false);
	let canManagePrograms = $derived(
		data.session?.capabilities.includes('manage_programs') ?? false
	);

	let step: 1 | 2 | 3 = $state(1);

	// ---- people

	interface Row {
		key: number;
		display_name: string;
		username: string;
		usernameEdited: boolean;
		role: Role;
		error: string;
	}
	let nextKey = 0;
	function blankRow(role: Role): Row {
		return { key: nextKey++, display_name: '', username: '', usernameEdited: false, role, error: '' };
	}
	let rows: Row[] = $state([blankRow('coordinator'), blankRow('trainer'), blankRow('trainee')]);
	let created: (CreatedUser & { role: Role })[] = $state([]);
	let creating = $state(false);

	/** A suggestion only: "Jordan Q. Trainer" → "jordan.trainer". */
	function suggestUsername(name: string): string {
		const parts = name
			.toLowerCase()
			.normalize('NFKD')
			.replace(/[^a-z\s-]/g, '')
			.split(/\s+/)
			.filter(Boolean);
		if (parts.length === 0) return '';
		return parts.length === 1 ? parts[0] : `${parts[0]}.${parts[parts.length - 1]}`;
	}
	function nameChanged(row: Row) {
		if (!row.usernameEdited) row.username = suggestUsername(row.display_name);
	}

	let filled = $derived(rows.filter((row) => row.display_name.trim() || row.username.trim()));

	async function createAll(event: SubmitEvent) {
		event.preventDefault();
		creating = true;
		try {
			// One at a time, in order: a refused row keeps its text and says why,
			// the others still go through.
			for (const row of filled) {
				row.error = '';
				try {
					const user = await createUser({
						username: row.username.trim(),
						display_name: row.display_name.trim(),
						employee_id: '',
						title: '',
						role: row.role
					});
					created.push({ ...user, role: row.role });
					rows = rows.filter((r) => r.key !== row.key);
				} catch (err) {
					row.error = err instanceof ApiError ? err.message : 'the server could not be reached';
				}
			}
			if (rows.length === 0) rows = [blankRow('trainee')];
		} finally {
			creating = false;
		}
	}

	const ROLE_NAMES: Record<Role, string> = {
		administrator: 'Administrator',
		coordinator: 'Coordinator',
		trainer: 'Trainer',
		trainee: 'Trainee'
	};

	// ---- program

	let imported: ImportedBody | null = $state(null);
	let importError = $state('');
	let importing = $state(false);
	async function useExample() {
		importError = '';
		importing = true;
		try {
			imported = await importProgram(JSON.stringify(example));
		} catch (err) {
			importError =
				err instanceof ApiError && err.code === 'name_taken'
					? 'The example program is already here; open it from Programs.'
					: err instanceof ApiError
						? err.message
						: 'the server could not be reached';
		} finally {
			importing = false;
		}
	}
</script>

<svelte:head>
	<title>Guided setup · Consolebook</title>
</svelte:head>

<h1>Guided setup</h1>
<p class="lede">
	Optional: add your people and start a program in a few minutes. You can leave
	at any time; nothing here is required, and everything can be changed later.
</p>

{#if !canManageUsers}
	<section class="card">
		<p>Guided setup is for administrators. <a href="/">Back to Home</a></p>
	</section>
{:else}
	<ol class="stepper" aria-label="Setup steps">
		<li aria-current={step === 1 ? 'step' : undefined}>
			<button type="button" class="linkish" onclick={() => (step = 1)}>1. People</button>
		</li>
		<li aria-current={step === 2 ? 'step' : undefined}>
			<button type="button" class="linkish" onclick={() => (step = 2)}>2. Program</button>
		</li>
		<li aria-current={step === 3 ? 'step' : undefined}>
			<button type="button" class="linkish" onclick={() => (step = 3)}>3. What's next</button>
		</li>
	</ol>

	{#if step === 1}
		<section class="panel">
			<h2>Add your people</h2>
			<p>
				A coordinator reviews and finalizes evaluations, a trainer writes them, and
				a trainee is evaluated. Add as many as you like; empty rows are skipped.
			</p>
			<form onsubmit={createAll}>
				<table class="grid people">
					<thead>
						<tr>
							<th>Name</th>
							<th>Username</th>
							<th>Role</th>
							<th><span class="visually-hidden">Remove</span></th>
						</tr>
					</thead>
					<tbody>
						{#each rows as row, i (row.key)}
							<tr>
								<td>
									<input
										aria-label={`Name, row ${i + 1}`}
										bind:value={row.display_name}
										oninput={() => nameChanged(row)}
										placeholder="Jordan Trainer"
									/>
								</td>
								<td>
									<input
										aria-label={`Username, row ${i + 1}`}
										bind:value={row.username}
										oninput={() => (row.usernameEdited = true)}
										placeholder="jordan.trainer"
										autocapitalize="none"
										spellcheck="false"
									/>
								</td>
								<td>
									<select aria-label={`Role, row ${i + 1}`} bind:value={row.role}>
										<option value="trainee">Trainee</option>
										<option value="trainer">Trainer</option>
										<option value="coordinator">Coordinator</option>
										<option value="administrator">Administrator</option>
									</select>
								</td>
								<td>
									<button
										type="button"
										class="secondary small"
										aria-label={`Remove row ${i + 1}`}
										onclick={() => (rows = rows.filter((r) => r.key !== row.key))}
									>
										Remove
									</button>
								</td>
							</tr>
							{#if row.error}
								<tr class="rowerror">
									<td colspan="4"><p class="error" role="alert">{row.username}: {row.error}</p></td>
								</tr>
							{/if}
						{/each}
					</tbody>
				</table>
				<div class="row actions">
					<button type="button" class="secondary" onclick={() => rows.push(blankRow('trainee'))}>
						Add another row
					</button>
					<button type="submit" disabled={creating || filled.length === 0}>
						{creating ? 'Creating…' : `Create ${filled.length} ${filled.length === 1 ? 'account' : 'accounts'}`}
					</button>
				</div>
			</form>
		</section>

		{#if created.length > 0}
			<section class="panel sheet" aria-labelledby="sheet-heading">
				<div class="cardhead">
					<h2 id="sheet-heading">Sign-in sheet</h2>
					<button type="button" class="secondary small noprint" onclick={() => window.print()}>
						Print
					</button>
				</div>
				<p class="warning">
					Codes are shown once and expire 15 minutes after they are made. Hand them
					out now. If one expires, issue a new one from Home under
					<em>Issue a password reset code</em>.
				</p>
				<p>
					Each person opens <strong>{window.location.origin}/reset</strong>, enters their
					username and code, and chooses their own password.
				</p>
				<table class="grid">
					<thead>
						<tr>
							<th>Name</th>
							<th>Role</th>
							<th>Username</th>
							<th>One-time code</th>
							<th>Valid until</th>
						</tr>
					</thead>
					<tbody>
						{#each created as person (person.id)}
							<tr>
								<td>{person.display_name}</td>
								<td>{ROLE_NAMES[person.role]}</td>
								<td><code>{person.username}</code></td>
								<td><code class="code">{person.reset_code}</code></td>
								<td class="nowrap">{instant(person.reset_expires_at)}</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</section>
		{/if}

		<p class="next noprint">
			<button type="button" onclick={() => (step = 2)}>Next: a program</button>
		</p>
	{:else if step === 2}
		<section class="panel">
			<h2>Start a training program</h2>
			{#if !canManagePrograms}
				<p>Your account cannot manage programs; ask an administrator who can.</p>
			{:else if imported}
				<p class="done" role="status">
					The example program is ready as an editable draft.
				</p>
				<p>
					Rename it, then replace its phases, competencies, tasks, scales, and forms
					with your agency's own. When it is right, publish it from the same page.
				</p>
				<p>
					<a class="button" href={`/programs/${imported.program_id}/versions/${imported.id}`}>
						Open the draft
					</a>
				</p>
			{:else}
				<div class="choices">
					<div class="choice">
						<h3>Start from an example</h3>
						<p>
							An invented telecommunicator program with four phases, four
							competencies, a 1–7 scale, a pass/fail check, and daily and weekly
							forms. You edit it before anyone uses it.
						</p>
						<button type="button" disabled={importing} onclick={useExample}>
							{importing ? 'Creating…' : 'Use the example'}
						</button>
					</div>
					<div class="choice">
						<h3>Start from scratch</h3>
						<p>
							Create an empty program and build it in the editor, or import a
							program file exported from another Consolebook.
						</p>
						<a class="button secondary" href="/programs">Go to Programs</a>
					</div>
				</div>
				{#if importError}
					<p class="error" role="alert">{importError}</p>
				{/if}
			{/if}
		</section>
		<p class="next">
			<button type="button" class="secondary" onclick={() => (step = 1)}>Back</button>
			<button type="button" onclick={() => (step = 3)}>Next: what's next</button>
		</p>
	{:else}
		<section class="panel">
			<h2>What's next</h2>
			<ol class="plain">
				<li><strong>Publish</strong> the program version once it is right. A published version is frozen; records made under it keep pointing at exactly that configuration.</li>
				<li><strong>Enroll</strong> a trainee in the published version, from the version's page.</li>
				<li><strong>Assign</strong> a trainer and <strong>record a training session</strong> on the trainee's enrollment page.</li>
				<li>The trainer writes the daily evaluation from their home page; a coordinator reviews and finalizes it; the trainee acknowledges it under <em>My records</em>.</li>
			</ol>
			<p>The <em>Getting started</em> checklist on Home tracks each of these as you go.</p>
			<p><a class="button" href="/">Back to Home</a></p>
		</section>
	{/if}
{/if}

<style>
	h3 {
		font-size: 1rem;
		margin: 0 0 0.4rem;
	}
	.stepper {
		display: flex;
		flex-wrap: wrap;
		gap: 0.25rem 1.25rem;
		list-style: none;
		padding: 0;
		margin: 0 0 1rem;
	}
	.stepper li[aria-current='step'] button {
		color: inherit;
		text-decoration: underline;
		text-underline-offset: 0.3em;
		text-decoration-thickness: 2px;
	}
	button.linkish {
		background: none;
		border: 0;
		padding: 0.25rem 0;
		color: light-dark(#2456a6, #7fabf5);
		font-weight: 600;
	}
	table.people input,
	table.people select {
		margin: 0;
		min-width: 9rem;
	}
	tr.rowerror td {
		border-bottom: 0;
		padding-top: 0;
	}
	.actions {
		margin-top: 1rem;
	}
	.next {
		display: flex;
		gap: 0.5rem;
	}
	.cardhead {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 0.75rem;
	}
	.warning {
		background: light-dark(#fdf1d7, #3b301e);
		border: 1px solid light-dark(#ecd49c, #6b5530);
		border-radius: 6px;
		padding: 0.6rem 0.8rem;
	}
	.done {
		font-weight: 600;
	}
	.choices {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(16rem, 1fr));
		gap: 1rem;
		margin: 0 0 1rem;
	}
	.choice {
		border: 1px solid light-dark(#d8dce3, #303642);
		border-radius: 8px;
		padding: 1rem;
	}
	a.button {
		display: inline-block;
		font-weight: 600;
		padding: 0.55rem 1.1rem;
		border-radius: 6px;
		background: #2456a6;
		color: #ffffff;
		text-decoration: none;
	}
	a.button.secondary {
		background: transparent;
		color: inherit;
		border: 1px solid light-dark(#b9c0cb, #4a5261);
	}
	ol.plain li {
		margin: 0 0 0.5rem;
	}
	code.code {
		font-size: 1.05em;
		letter-spacing: 0.02em;
	}
	.visually-hidden {
		position: absolute;
		width: 1px;
		height: 1px;
		overflow: hidden;
		clip-path: inset(50%);
		white-space: nowrap;
	}
	/* The sheet prints alone: no navigation, steps, or form. */
	@media print {
		:global(header),
		:global(footer),
		h1,
		.lede,
		.stepper,
		.noprint,
		section.panel:not(.sheet) {
			display: none !important;
		}
		section.sheet {
			border: 0;
			padding: 0;
		}
	}
</style>
