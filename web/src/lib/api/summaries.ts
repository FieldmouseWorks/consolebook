import { request } from './transport';

// Weekly summaries and task signoffs (Milestone 4 slice 4; ADR 0013).

export interface SummaryLink {
	daily_version_id: number;
	record_id: number;
	version_number: number;
	content_hash: string;
	finalized_at: number;
	form_name: string | null;
	business_date: string | null;
}

export function summaryForms(
	enrollmentId: number
): Promise<{ forms: { id: number; name: string }[] }> {
	return request(`/api/enrollments/${enrollmentId}/summary-forms`);
}

export function createWeeklySummary(
	enrollmentId: number,
	formId?: number
): Promise<{ id: number }> {
	return request(`/api/enrollments/${enrollmentId}/weekly-summary`, {
		method: 'POST',
		body: JSON.stringify({ evaluation_form_id: formId })
	});
}

export function linkableDailies(draftId: number): Promise<{ dailies: SummaryLink[] }> {
	return request(`/api/drafts/${draftId}/linkable-dailies`);
}

export function addSummaryLink(
	draftId: number,
	dailyVersionId: number,
	revision: number
): Promise<{ revision: number }> {
	return request(`/api/drafts/${draftId}/links`, {
		method: 'POST',
		body: JSON.stringify({ daily_version_id: dailyVersionId, revision })
	});
}

export function removeSummaryLink(
	draftId: number,
	dailyVersionId: number,
	revision: number
): Promise<{ revision: number }> {
	return request(`/api/drafts/${draftId}/links/remove`, {
		method: 'POST',
		body: JSON.stringify({ daily_version_id: dailyVersionId, revision })
	});
}
