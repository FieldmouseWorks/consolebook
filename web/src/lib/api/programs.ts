import { request } from './transport';

// Program configuration (docs/formats/program-version-export.md documents
// the content document; field names mirror the server verbatim).

export type TransitionKind = 'advance' | 'remediation' | 'skip' | 'restart';
export type ScaleKind = 'anchored_numeric' | 'pass_fail' | 'narrative_only';
export type RecordType = 'daily_report' | 'weekly_summary' | 'phase_evaluation';

export interface CitationDef {
	body: string;
	edition: string;
	clause: string;
	note: string;
}

export interface PhaseDef {
	name: string;
	description: string;
	presentation_number: number;
}

export interface TransitionDef {
	from_phase: string;
	to_phase: string;
	kind: TransitionKind;
}

export interface TaskDef {
	prompt: string;
	citations: CitationDef[];
}

export interface CompetencyDef {
	category: string;
	name: string;
	description: string;
	tasks: TaskDef[];
	citations: CitationDef[];
}

export interface AnchorDef {
	value: number;
	label: string;
	definition: string;
}

export interface ScaleDef {
	name: string;
	kind: ScaleKind;
	min_value: number | null;
	max_value: number | null;
	anchors: AnchorDef[];
}

export interface ModifierDef {
	code: string;
	label: string;
	description: string;
}

export interface FormCompetencyDef {
	competency: string;
	rating_scale: string;
}

export interface NarrativeDef {
	prompt: string;
	required: boolean;
}

export interface FormDef {
	record_type: RecordType;
	name: string;
	instructions: string;
	competencies: FormCompetencyDef[];
	narratives: NarrativeDef[];
}

export interface PolicyDef {
	review_approved: boolean;
	required_narratives: boolean;
	ratings_complete: boolean;
}

export interface VersionContent {
	name: string;
	label: string;
	description: string;
	phases: PhaseDef[];
	phase_transitions: TransitionDef[];
	competencies: CompetencyDef[];
	rating_scales: ScaleDef[];
	rating_modifiers: ModifierDef[];
	evaluation_forms: FormDef[];
	citations: CitationDef[];
	finalization_policy: PolicyDef;
}

export interface ProgramSummary {
	id: number;
	name: string;
	created_at: number;
}

export interface VersionSummary {
	id: number;
	program_id: number;
	version_number: number;
	label: string;
	name: string;
	created_at: number;
	published_at: number | null;
}

export interface ProgramsBody {
	programs: ProgramSummary[];
}

export function listPrograms(): Promise<ProgramsBody> {
	return request('/api/programs');
}

export function createProgram(name: string): Promise<{ id: number }> {
	return request('/api/programs', { method: 'POST', body: JSON.stringify({ name }) });
}

export interface VersionsBody {
	program: ProgramSummary;
	versions: VersionSummary[];
}

export function getProgramVersions(programId: number): Promise<VersionsBody> {
	return request(`/api/programs/${programId}/versions`);
}

export function createVersion(
	programId: number,
	content: VersionContent
): Promise<{ id: number }> {
	return request(`/api/programs/${programId}/versions`, {
		method: 'POST',
		body: JSON.stringify(content)
	});
}

export interface VersionBody {
	summary: VersionSummary;
	content: VersionContent;
}

export function getVersion(versionId: number): Promise<VersionBody> {
	return request(`/api/program-versions/${versionId}`);
}

export function replaceVersionContent(
	versionId: number,
	content: VersionContent
): Promise<void> {
	return request(`/api/program-versions/${versionId}/content`, {
		method: 'PUT',
		body: JSON.stringify(content)
	});
}

export function publishVersion(versionId: number): Promise<void> {
	return request(`/api/program-versions/${versionId}/publish`, {
		method: 'POST',
		body: JSON.stringify({})
	});
}

export function discardVersion(versionId: number): Promise<void> {
	return request(`/api/program-versions/${versionId}`, { method: 'DELETE' });
}

/** Download URL for a version's export document (a browser navigation). */
export function versionExportPath(versionId: number): string {
	return `/api/program-versions/${versionId}/export`;
}

export interface ImportedBody {
	id: number;
	program_id: number;
}

export function importProgram(document: string): Promise<ImportedBody> {
	return request('/api/programs/import', {
		method: 'POST',
		body: JSON.stringify({ document })
	});
}

export function importNextVersion(
	programId: number,
	document: string
): Promise<ImportedBody> {
	return request(`/api/programs/${programId}/versions/import`, {
		method: 'POST',
		body: JSON.stringify({ document })
	});
}

/** A structurally valid empty draft for starting a program from scratch. */
export function blankContent(name: string): VersionContent {
	return {
		name,
		label: '',
		description: '',
		phases: [],
		phase_transitions: [],
		competencies: [],
		rating_scales: [],
		rating_modifiers: [],
		evaluation_forms: [],
		citations: [],
		finalization_policy: {
			review_approved: true,
			required_narratives: true,
			ratings_complete: true
		}
	};
}
