<script lang="ts">
	// Read-only presentation of a trainee's own task-signoff state and
	// complete history (issue #49; ADR 0021). No control here records,
	// overrides, revokes, or contests anything: recording stays on the
	// enrollment page under its own authority. Every row is the stored act,
	// with the signer's recorded name and instant.
	import { ApiError } from '$lib/api/transport';
	import {
		ownSignoffHistory,
		signoffStateLabel,
		type SignoffHistory,
		type SignoffKind,
		type SignoffTaskRow
	} from '$lib/api/signoffs';
	import { instant } from '$lib/format';

	let { enrollmentId }: { enrollmentId: number } = $props();

	let history: SignoffHistory | null = $state(null);
	let loading = $state(true);
	let error = $state('');
	/** The enrollment whose response may still land; anything older is dropped.
	 * Deliberately not reactive state: the effect reads it only to invalidate. */
	let current = 0;

	$effect(() => {
		const wanted = enrollmentId;
		const run = ++current;
		history = null;
		loading = true;
		error = '';
		ownSignoffHistory(wanted).then(
			(body) => {
				if (run !== current || body.enrollment_id !== wanted) {
					return;
				}
				history = body;
				loading = false;
			},
			(err: unknown) => {
				if (run !== current) {
					return;
				}
				error =
					err instanceof ApiError ? err.message : 'the server could not be reached';
				loading = false;
			}
		);
		return () => {
			// A response nobody is waiting for must not revive the spinner.
			if (current === run) {
				current += 1;
			}
		};
	});

	/** Every task of the version the enrollment pins now, signed or not. */
	let currentTasks: SignoffTaskRow[] = $derived.by(() => {
		const loaded = history;
		return loaded === null
			? []
			: loaded.tasks.filter((task) => task.in_current_version);
	});
	let currentUnsigned = $derived(
		currentTasks.filter((task) => task.current_kind === null).length
	);
	/** Tasks whose signoffs were recorded under an earlier version. */
	let earlierTasks: SignoffTaskRow[] = $derived.by(() => {
		const loaded = history;
		return loaded === null
			? []
			: loaded.tasks.filter((task) => !task.in_current_version);
	});

	function kindLabel(kind: SignoffKind): string {
		switch (kind) {
			case 'observed':
				return 'Observed';
			case 'demonstrated':
				return 'Demonstrated';
			case 'revoked':
				return 'Revoked';
		}
	}

	/** The state pill's class, so unsigned, signed, and revoked differ. */
	function stateClass(kind: SignoffKind | null): string {
		switch (kind) {
			case 'observed':
			case 'demonstrated':
				return 'signed';
			case 'revoked':
				return 'revoked';
			case null:
				return 'unsigned';
		}
	}
</script>

{#if loading}
	<p class="quiet" role="status">Loading your signoffs…</p>
{:else if error}
	<p class="error" role="alert">{error}</p>
{:else if history === null}
	<p class="quiet">No signoff history is available for this enrollment.</p>
{:else}
	<p class="quiet">
		{history.current_program_version_label || 'This program version'}
		— v{history.current_program_version_number}.
		{#if currentTasks.length === 0}
			This version records no tasks.
		{:else if currentUnsigned === 0}
			Every task of this version has a signoff.
		{:else}
			{currentUnsigned} of {currentTasks.length}
			{currentTasks.length === 1 ? 'task has' : 'tasks have'} no signoff.
		{/if}
	</p>

	{#if currentTasks.length > 0}
		<table class="grid">
			<thead>
				<tr>
					<th>Task</th>
					<th>Current state</th>
					<th>Latest signoff</th>
				</tr>
			</thead>
			<tbody>
				{#each currentTasks as task (task.task_id)}
					<tr>
						<td>
							<strong>{task.competency_name}</strong>
							{#if task.competency_category}
								<span class="quiet-inline">({task.competency_category})</span>
							{/if}
							<p class="quiet prompt">{task.prompt}</p>
						</td>
						<td>
							<span class={`pill ${stateClass(task.current_kind)}`}>
								{signoffStateLabel(task)}
							</span>
						</td>
						<td>
							{#if task.signoffs.length === 0}
								<span class="quiet-inline">—</span>
							{:else}
								{@const latest = task.signoffs[task.signoffs.length - 1]}
								<p class="quiet prompt">
									{instant(latest.signed_at)} · {latest.signed_by_display_name}
									{#if latest.reason}
										— {latest.reason}
									{/if}
								</p>
							{/if}
						</td>
					</tr>
					{#if task.signoffs.length > 1}
						<tr>
							<td colspan="3">
								<details>
									<summary>
										All {task.signoffs.length} recorded signoffs for this task
									</summary>
									<ol class="history">
										{#each task.signoffs as entry (entry.signoff_id)}
											<li>
												<strong>{kindLabel(entry.kind)}</strong>
												<span class="quiet-inline">
													{instant(entry.signed_at)} · {entry.signed_by_display_name}
												</span>
												{#if entry.reason}
													<p class="reason">{entry.reason}</p>
												{/if}
											</li>
										{/each}
									</ol>
								</details>
							</td>
						</tr>
					{/if}
				{/each}
			</tbody>
		</table>
	{:else if earlierTasks.length === 0}
		<p class="quiet">Nothing has been signed off for this enrollment.</p>
	{/if}

	{#if earlierTasks.length > 0}
		<h3>Earlier program versions</h3>
		<p class="quiet">
			Signoffs recorded under a version this enrollment no longer pins.
			They stay part of your history and keep the version they were
			signed under.
		</p>
		<ul class="earlier-tasks">
			{#each earlierTasks as task (task.task_id)}
				<li>
					<strong>{task.competency_name}</strong>
					<span class="quiet-inline">
						· {task.program_version_label || 'earlier version'}
						(v{task.program_version_number})
					</span>
					<p class="quiet prompt">{task.prompt}</p>
					<p class="quiet prompt">
						{signoffStateLabel(task)}
						{#if task.signoffs.length > 0}
							{@const latest = task.signoffs[task.signoffs.length - 1]}
							· {instant(latest.signed_at)} · {latest.signed_by_display_name}
							{#if latest.reason}
								— {latest.reason}
							{/if}
						{/if}
					</p>
					{#if task.signoffs.length > 1}
						<details>
							<summary>
								All {task.signoffs.length} recorded signoffs for this task
							</summary>
							<ol class="history">
								{#each task.signoffs as entry (entry.signoff_id)}
									<li>
										<strong>{kindLabel(entry.kind)}</strong>
										<span class="quiet-inline">
											{instant(entry.signed_at)} · {entry.signed_by_display_name}
										</span>
										{#if entry.reason}
											<p class="reason">{entry.reason}</p>
										{/if}
									</li>
								{/each}
							</ol>
						</details>
					{/if}
				</li>
			{/each}
		</ul>
	{/if}
{/if}

<style>
	.quiet {
		opacity: 0.7;
	}
	.quiet-inline {
		opacity: 0.7;
		font-size: 0.85rem;
	}
	.prompt {
		margin: 0.15rem 0 0;
		font-size: 0.9rem;
	}
	.error {
		color: #a33;
	}
	.pill.revoked {
		background: light-dark(#fdeaea, #3b2323);
		color: light-dark(#8c2f2f, #e79b9b);
	}
	.pill.signed {
		background: light-dark(#e8f6ec, #22352a);
		color: light-dark(#28633c, #8fd3a5);
	}
	.pill.unsigned {
		background: light-dark(#fdf1d7, #3b301e);
		color: light-dark(#7a5410, #e4c26d);
	}
	details summary {
		cursor: pointer;
		font-size: 0.9rem;
	}
	ol.history {
		margin: 0.4rem 0 0.2rem;
		padding-left: 1.2rem;
	}
	ol.history li {
		padding: 0.15rem 0;
	}
	.reason {
		margin: 0.15rem 0 0.3rem;
		padding: 0.3rem 0.5rem;
		background: light-dark(#f4f6f8, #232833);
		border-left: 3px solid #b9c2cc;
		white-space: pre-wrap;
	}
	ul.earlier-tasks {
		list-style: none;
		padding: 0;
		margin: 0.5rem 0 0;
	}
	ul.earlier-tasks li {
		border-top: 1px solid light-dark(#e3e6eb, #2a303b);
		padding: 0.5rem 0;
	}
</style>
