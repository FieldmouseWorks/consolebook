import { request } from './transport';
import type { PolicyDef } from './programs';

export interface VersionMeta {
	version_number: number;
	record_schema: number;
	content_hash: string;
	chain_hash: string;
	finalized_at: number;
	finalized_by: number;
	finalized_by_display_name: string;
}

/** One identity as the finalized envelope presents it. */
export interface EnvelopeUser {
	id: number;
	username: string;
	display_name: string;
}

/** The stored canonical envelope (record schema 1, ADR 0011). */
export interface RecordEnvelope {
	attachments: unknown[];
	attribution: {
		kind: string;
		actor: EnvelopeUser;
		to: EnvelopeUser | null;
		recorded_at: number;
	}[];
	canonicalization: string;
	content: {
		narratives: { prompt: string; required: boolean; text: string | null }[];
		ratings: {
			competency: { category: string; name: string; description: string; tasks: string[] };
			scale: {
				name: string;
				kind: string;
				min_value: number | null;
				max_value: number | null;
				anchors: { value: number; label: string; definition: string }[];
			};
			value: number | null;
			not_observed: boolean;
			modifiers: { code: string; label: string; description: string }[];
		}[];
	};
	finalization: {
		finalized_at: number;
		finalized_by: EnvelopeUser;
		policy: PolicyDef;
	};
	form: { name: string; instructions: string; record_type: string };
	instance: string;
	program: { name: string; version_number: number; label: string };
	/** Schema 2 (ADR 0013): the exact daily versions a summary covers. */
	daily_reports?: { content_hash: string; record_id: number; version_number: number }[];
	record: {
		id: number;
		version_number: number;
		record_schema: number;
		predecessor_content_hash: string | null;
	};
	review: { reviewer: EnvelopeUser; decision: string; comment: string; decided_at: number }[];
	sessions: {
		business_date: string;
		timezone: string;
		local_start: string;
		local_end: string | null;
		utc_start: number;
		utc_end: number | null;
		disposition: string | null;
		phase: { name: string; presentation_number: number } | null;
		trainers: EnvelopeUser[];
	}[];
	trainee: EnvelopeUser & { employee_id: string; title: string };
}

export interface FinalizedView {
	meta: VersionMeta;
	envelope: RecordEnvelope;
}

export interface Verification {
	content_hash_ok: boolean;
	chain_hash_ok: boolean;
}

export function finalizeDraft(draftId: number, revision: number): Promise<VersionMeta> {
	return request(`/api/drafts/${draftId}/finalize`, {
		method: 'POST',
		body: JSON.stringify({ revision })
	});
}

export function finalizedVersion(draftId: number): Promise<FinalizedView> {
	return request(`/api/drafts/${draftId}/version`);
}

export function verifyVersion(draftId: number): Promise<Verification> {
	return request(`/api/drafts/${draftId}/version/verify`);
}

// Acknowledgments and the trainee's own-records timeline (Milestone 4
// slice 2). Acknowledgment means receipt, not agreement.

export type TraineeAckKind = 'acknowledged' | 'acknowledged_with_response' | 'refused';
export type AttestedKind = 'supervisor_attested_refusal' | 'unavailable';
export type AckKind = TraineeAckKind | AttestedKind;

export interface Acknowledgment {
	kind: AckKind;
	response: string;
	user_display_name: string;
	recorded_by: number;
	recorded_by_display_name: string;
	recorded_at: number;
}

export interface TimelineRecord {
	record_id: number;
	program_name: string;
	version_number: number;
	form_name: string;
	business_date: string | null;
	finalized_at: number;
	/** The latest version's number; above 1 the record was amended. */
	record_version_number: number;
	acknowledgment_kind: AckKind | null;
	acknowledged_at: number | null;
}

// Amendments (Milestone 4 slice 3): a correction produces a successor
// version linked to the original with a reason and authority; the
// original stays readable while retained.

export interface AmendmentView {
	reason: string;
	opened_by_display_name: string;
	opened_at: number;
}

export interface VersionHistoryRow {
	version_number: number;
	record_schema: number;
	content_hash: string;
	chain_hash: string;
	finalized_at: number;
	finalized_by_display_name: string;
	amendment: AmendmentView | null;
	acknowledgment: Acknowledgment | null;
}

export function amendRecord(draftId: number, reason: string): Promise<void> {
	return request(`/api/drafts/${draftId}/amend`, {
		method: 'POST',
		body: JSON.stringify({ reason })
	});
}

export function versionHistory(
	draftId: number
): Promise<{ versions: VersionHistoryRow[] }> {
	return request(`/api/drafts/${draftId}/versions`);
}

/** One retained version by number — a superseded original included. */
export function finalizedVersionAt(
	draftId: number,
	versionNumber: number
): Promise<FinalizedView> {
	return request(`/api/drafts/${draftId}/versions/${versionNumber}`);
}

export function verifyVersionAt(
	draftId: number,
	versionNumber: number
): Promise<Verification> {
	return request(`/api/drafts/${draftId}/versions/${versionNumber}/verify`);
}

export function acknowledgeRecord(
	draftId: number,
	kind: TraineeAckKind,
	response?: string
): Promise<void> {
	return request(`/api/drafts/${draftId}/acknowledge`, {
		method: 'POST',
		body: JSON.stringify({ kind, response })
	});
}

export function attestRecord(
	draftId: number,
	kind: AttestedKind,
	reason: string
): Promise<void> {
	return request(`/api/drafts/${draftId}/attest`, {
		method: 'POST',
		body: JSON.stringify({ kind, reason })
	});
}

export function getAcknowledgment(
	draftId: number
): Promise<{ acknowledgment: Acknowledgment | null }> {
	return request(`/api/drafts/${draftId}/acknowledgment`);
}

export function myRecords(): Promise<{ records: TimelineRecord[] }> {
	return request('/api/my/records');
}
