<script lang="ts">
	// Backup health for administrators (ADR 0024). Copies off the server are
	// pressed for, not required: the warning stays until they are current.
	import { BACKUP_DIR_COMMAND, type BackupStatus } from '#lib/api.js';
	import { instant } from '#lib/format.js';

	let { status }: { status: BackupStatus } = $props();

	let tone = $derived(
		status.copy.state === 'current'
			? 'ok'
			: status.copy.state === 'unreachable'
				? 'bad'
				: 'warn'
	);
</script>

<div class="health {tone}" role={tone === 'ok' ? 'status' : 'alert'}>
	{#if status.copy.state === 'current'}
		<p class="headline">Backups are copied off this server.</p>
		<p>
			The newest backup is also in <code>{status.copy.directory}</code>
			({status.copy.count} {status.copy.count === 1 ? 'copy' : 'copies'} kept there).
		</p>
	{:else if status.copy.state === 'not_configured'}
		<p class="headline">Backups are only on this server's own disk.</p>
		<p>
			If that disk fails, the records and every backup are lost together. Mount a
			second disk or a network share on the server, then run:
		</p>
		<pre><code>{BACKUP_DIR_COMMAND}</code></pre>
	{:else if status.copy.state === 'unreachable'}
		<p class="headline">The backup copy location cannot be reached.</p>
		<p>
			<code>{status.copy.directory}</code>: {status.copy.problem}. Check that it is
			mounted and writable by the <code>consolebook</code> account. Local backups
			continue meanwhile.
		</p>
	{:else}
		<p class="headline">The off-server copy is behind.</p>
		<p>
			<code>{status.copy.directory}</code> does not have the newest backup yet. It is
			copied within a minute of each backup and retried every 15 minutes; if this
			persists, check that the location is mounted and writable.
		</p>
	{/if}
</div>

<dl class="facts">
	<dt>Newest backup</dt>
	<dd>
		{status.local.newest_at !== null ? instant(status.local.newest_at) : 'none yet (the first runs within a minute of starting)'}
	</dd>
	<dt>Schedule</dt>
	<dd>Every {status.interval_hours} hours, keeping {status.keep}</dd>
	<dt>On this server</dt>
	<dd><code>{status.local.directory}</code> ({status.local.count} kept)</dd>
</dl>

<style>
	.health {
		border-radius: 6px;
		padding: 0.75rem 0.9rem;
		margin: 0 0 1rem;
		border: 1px solid;
	}
	.health.ok {
		background: light-dark(#eef4ee, #1e2a1f);
		border-color: light-dark(#bcd4bc, #3c553d);
	}
	.health.warn {
		background: light-dark(#fdf1d7, #3b301e);
		border-color: light-dark(#ecd49c, #6b5530);
	}
	.health.bad {
		background: light-dark(#fbe9e9, #3a1d20);
		border-color: light-dark(#e4b4b4, #7c3b41);
	}
	.headline {
		font-weight: 700;
	}
	p {
		margin: 0 0 0.4rem;
	}
	pre {
		margin: 0.4rem 0 0;
		padding: 0.5rem 0.6rem;
		background: light-dark(#ffffff, #141821);
		border-radius: 4px;
		overflow-x: auto;
		font-size: 0.82rem;
	}
	code {
		overflow-wrap: anywhere;
	}
</style>
