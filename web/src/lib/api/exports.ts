import { ApiError, request, requireOk } from './transport';

// Record exports (Milestone 5 slice 1; docs/formats/record-export.md;
// ADR 0014): the stored canonical bytes travel verbatim beside
// manifests, and an archive verifies from its own contents alone.

export interface ExportSummary {
	installation_id: string;
	record_count: number;
	version_count: number;
	disposed_record_count: number;
	boundary_checkpoint_count: number;
}

export function exportSummary(): Promise<ExportSummary> {
	return request('/api/exports/summary');
}

/** Download path for one finalized version. */
export function recordVersionExportPath(recordId: number, versionNumber: number): string {
	return `/api/drafts/${recordId}/versions/${versionNumber}/export`;
}

/** Download path for every retained version of a record. */
export function recordExportPath(recordId: number): string {
	return `/api/drafts/${recordId}/export`;
}

/** Download path for every finalized version of an enrollment's records. */
export function enrollmentExportPath(enrollmentId: number): string {
	return `/api/enrollments/${enrollmentId}/export`;
}

/** Download path for every finalized version the installation holds. */
export function installationExportPath(): string {
	return '/api/exports/records';
}

/**
 * Download path for the complete installation export: everything the
 * installation holds except credentials (docs/formats/installation-export.md;
 * ADR 0028).
 */
export function completeInstallationExportPath(): string {
	return '/api/exports/installation';
}

/**
 * Thrown when an export response started (`2xx`) and then ended before the
 * archive finished transferring (ADR 0014; #129). Distinguished from a
 * connection failure before any response arrived so the operator sees that
 * the transfer was cut off rather than that the server was unreachable.
 */
export class IncompleteExportError extends Error {
	constructor(cause?: unknown) {
		super('the export did not finish; download it again', { cause });
		this.name = 'IncompleteExportError';
	}
}

/**
 * Operator-facing message for an error thrown by `downloadExport`:
 * - typed server refusal (`ApiError`): the server's own message;
 * - transfer cut off mid-body (`IncompleteExportError`): incomplete-transfer guidance;
 * - connection failure before a response arrived: unreachable server.
 */
export function describeExportError(err: unknown): string {
	if (err instanceof ApiError || err instanceof IncompleteExportError) {
		return err.message;
	}
	return 'the server could not be reached';
}

/**
 * Fetches an export archive and hands it to the browser as a download, so
 * a refusal surfaces as an error instead of a saved error document.
 * Returns the server's file name.
 *
 * The server streams its archive, but this path still reads the whole
 * response before handing the browser a Blob. That keeps the operator's
 * refusal handling intact — a typed refusal arrives as a JSON body before
 * anything is saved, and a transfer that fails part way rejects with
 * `IncompleteExportError` rather than saving a truncated file — at the cost
 * of holding the archive in the browser while it downloads (#47, #129).
 */
export async function downloadExport(path: string): Promise<string> {
	const response = await fetch(path);
	await requireOk(response);
	const disposition = response.headers.get('Content-Disposition') ?? '';
	const named = /filename="([^"]+)"/.exec(disposition);
	const fileName = named?.[1] ?? 'consolebook-export.zip';
	let blob: Blob;
	try {
		blob = await response.blob();
	} catch (err) {
		throw new IncompleteExportError(err);
	}
	const url = URL.createObjectURL(blob);
	const anchor = document.createElement('a');
	anchor.href = url;
	anchor.download = fileName;
	document.body.append(anchor);
	anchor.click();
	anchor.remove();
	// Revoke once the browser has had time to start the download.
	setTimeout(() => URL.revokeObjectURL(url), 60_000);
	return fileName;
}
