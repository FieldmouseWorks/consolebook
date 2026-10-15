<script lang="ts">
	// The home page's installation exports card, shown only to holders of the
	// export_records capability. `busy` is the page's shared flag: either
	// export disables every other page action while it runs, and they it.
	import {
		ApiError,
		completeInstallationExportPath,
		downloadExport,
		exportSummary,
		installationExportPath,
		type ExportSummary
	} from '$lib/api';

	let { busy = $bindable() }: { busy: boolean } = $props();

	let summary: ExportSummary | null = $state(null);
	$effect(() => {
		exportSummary().then(
			(body) => (summary = body),
			() => (summary = null)
		);
	});

	// Every finalized version leaves as one records-only archive of its
	// stored bytes, verifiable from the archive alone (ADR 0014). The
	// complete installation export is below. Both are the explicit
	// export_records authority.
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

	// Everything the installation holds except credentials, as one archive
	// that verifies from the file alone (ADR 0028). Offered even before any
	// record is finalized: people, programs, and the audit trail still leave
	// with it. Its own messages, so neither export overwrites the other's.
	let wholeExportError = $state('');
	let wholeExported = $state('');
	async function exportWholeInstallation() {
		wholeExportError = '';
		wholeExported = '';
		busy = true;
		try {
			wholeExported = await downloadExport(completeInstallationExportPath());
		} catch (err) {
			wholeExportError =
				err instanceof ApiError ? err.message : 'the server could not be reached';
		} finally {
			busy = false;
		}
	}
</script>

<section class="card">
	<h2>Installation exports</h2>
	<h3>Finalized records</h3>
	{#if summary === null}
		<p class="quiet">Checking finalized records…</p>
	{:else if summary.version_count === 0}
		<p class="quiet">No finalized records yet; there is no record export to make.</p>
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
	<h3>The whole installation</h3>
	<p>
		The installation export carries everything this installation holds except
		credentials: records, people, programs, enrollments, sessions, drafts and
		their reviews, retention, notices, and the audit trail.
		<code>consolebook-server export verify</code> checks it from the file alone.
	</p>
	<p>
		It holds every person's data and is not encrypted. Store it as the
		installation's own data.
	</p>
	<button type="button" class="secondary" disabled={busy} onclick={exportWholeInstallation}>
		Export the whole installation
	</button>
	{#if wholeExported}
		<p class="quiet" role="status">Downloaded {wholeExported}.</p>
	{/if}
	{#if wholeExportError}
		<p class="error" role="alert">{wholeExportError}</p>
	{/if}
</section>

<style>
	/* The home page's scoped rules for these elements, carried with the card. */
	h2 {
		font-size: 1.1rem;
		margin: 0 0 0.5rem;
	}
	code {
		font-size: 1.05em;
	}
	.quiet {
		opacity: 0.7;
		margin: 0;
	}
</style>
