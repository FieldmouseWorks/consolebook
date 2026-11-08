import { request } from './transport';
import type { RecordType } from './programs';
import type { AmendmentView } from './records';
import type { SummaryLink } from './summaries';

export type DraftStatus =
	| 'draft'
	| 'submitted'
	| 'changes_requested'
	| 'returned'
	| 'approved'
	| 'finalized';

export type ReviewDecisionKind = 'approved' | 'changes_requested' | 'returned';

export interface ReviewDecision {
	id: number;
	reviewer_user_id: number;
	reviewer_display_name: string;
	decision: ReviewDecisionKind;
	comment: string;
	decided_at: number;
}

export interface ReviewQueueRow {
	record_id: number;
	trainee_user_id: number;
	trainee_display_name: string;
	owner_display_name: string;
	program_name: string;
	version_number: number;
	submitted_at: number;
	eligible: boolean;
}

export interface ContributorEvent {
	id: number;
	kind: string;
	actor_user_id: number;
	actor_display_name: string;
	to_user_id: number | null;
	to_display_name: string | null;
	recorded_at: number;
}

export interface CoveredSession {
	session_id: number;
	business_date: string;
	timezone: string;
	local_start: string;
	local_end: string | null;
}

export interface SnapshotMeta {
	id: number;
	reason: string;
	taken_at: number;
	taken_by: number | null;
}

export interface EligibleRecipient {
	user_id: number;
	display_name: string;
}

export interface SkeletonAnchor {
	value: number;
	label: string;
	definition: string;
}

export interface SkeletonCompetency {
	form_competency_id: number;
	category: string;
	name: string;
	description: string;
	scale_name: string;
	scale_kind: 'anchored_numeric' | 'pass_fail' | 'narrative_only';
	min_value: number | null;
	max_value: number | null;
	anchors: SkeletonAnchor[];
}

export interface SkeletonNarrative {
	form_narrative_id: number;
	prompt: string;
	required: boolean;
}

export interface SkeletonModifier {
	rating_modifier_id: number;
	code: string;
	label: string;
	description: string;
}

export interface RatingEntry {
	form_competency_id: number;
	value: number | null;
	not_observed: boolean;
	modifier_ids: number[];
}

export interface NarrativeEntry {
	form_narrative_id: number;
	text: string;
}

export interface DraftContent {
	ratings: RatingEntry[];
	narratives: NarrativeEntry[];
}

export interface DraftView {
	id: number;
	enrollment_id: number;
	program_version_id: number;
	evaluation_form_id: number;
	owner_user_id: number;
	owner_display_name: string;
	status: DraftStatus;
	trainee_user_id: number;
	trainee_display_name: string;
	program_name: string;
	version_number: number;
	sessions: CoveredSession[];
	events: ContributorEvent[];
	snapshots: SnapshotMeta[];
	eligible_recipients: EligibleRecipient[];
	decisions: ReviewDecision[];
	record_type: RecordType;
	summary_links: SummaryLink[];
	viewer_may_review: boolean;
	viewer_may_finalize: boolean;
	viewer_may_amend: boolean;
	latest_version_number: number | null;
	open_amendment: AmendmentView | null;
	created_at: number;
	revision: number;
	form: {
		form_name: string;
		instructions: string;
		competencies: SkeletonCompetency[];
		narratives: SkeletonNarrative[];
		modifiers: SkeletonModifier[];
	};
	content: DraftContent;
}

export function createDraft(sessionId: number, formId?: number): Promise<{ id: number }> {
	return request(`/api/sessions/${sessionId}/draft`, {
		method: 'POST',
		body: JSON.stringify({ evaluation_form_id: formId })
	});
}

export function dailyForms(
	sessionId: number
): Promise<{ forms: { id: number; name: string }[] }> {
	return request(`/api/sessions/${sessionId}/daily-forms`);
}

export function getDraft(draftId: number): Promise<DraftView> {
	return request(`/api/drafts/${draftId}`);
}

export function saveDraftContent(
	draftId: number,
	revision: number,
	content: DraftContent
): Promise<{ revision: number }> {
	return request(`/api/drafts/${draftId}/content`, {
		method: 'PUT',
		body: JSON.stringify({ revision, ...content })
	});
}

export function transferDraft(draftId: number, toUserId: number): Promise<void> {
	return request(`/api/drafts/${draftId}/transfer`, {
		method: 'POST',
		body: JSON.stringify({ to_user_id: toUserId })
	});
}

export function submitDraft(draftId: number, revision: number): Promise<void> {
	return request(`/api/drafts/${draftId}/submit`, {
		method: 'POST',
		body: JSON.stringify({ revision })
	});
}

export function reviewDraft(
	draftId: number,
	decision: ReviewDecisionKind,
	comment?: string
): Promise<void> {
	return request(`/api/drafts/${draftId}/review`, {
		method: 'POST',
		body: JSON.stringify({ decision, comment })
	});
}

export function reviewQueue(): Promise<{ drafts: ReviewQueueRow[] }> {
	return request('/api/reviews/queue');
}
