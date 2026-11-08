// Typed task-signoff recording, matrix, and retained-history contracts.
import { request } from './transport';

/** The closed signoff kind set (ADR 0013). */
export type SignoffKind = 'observed' | 'demonstrated' | 'revoked';

/** One retained signoff row, exactly as stored. */
export interface SignoffEntry {
	signoff_id: number;
	kind: SignoffKind;
	/** The override's recorded reason; empty for a first signoff. */
	reason: string;
	signed_by_user_id: number;
	/** The signer's name as recorded at the act, never a live rename. */
	signed_by_display_name: string;
	signed_at: number;
}

/** One task with every signoff recorded against it, oldest first. */
export interface SignoffTaskRow {
	task_id: number;
	prompt: string;
	competency_category: string;
	competency_name: string;
	program_version_id: number;
	program_version_number: number;
	program_version_label: string;
	/** Whether the task belongs to the enrollment's currently pinned version. */
	in_current_version: boolean;
	/** The latest row's kind, or null when nothing was ever signed off. */
	current_kind: SignoffKind | null;
	signoffs: SignoffEntry[];
}

/** The complete retained signoff history of one enrollment. */
export interface SignoffHistory {
	enrollment_id: number;
	trainee_user_id: number;
	current_program_version_id: number;
	current_program_version_number: number;
	current_program_version_label: string;
	tasks: SignoffTaskRow[];
}

/**
 * The trainee's own complete signoff state and history. Admitted for the
 * readers the enrollment's training history is open to, and for the
 * trainee holding `view_own_records` on their own enrollment; every other
 * actor is refused typed.
 */
export function ownSignoffHistory(enrollmentId: number): Promise<SignoffHistory> {
	return request(`/api/enrollments/${enrollmentId}/signoff-history`);
}

/** A task's current state in words, distinguishing revoked from unsigned. */
export function signoffStateLabel(task: SignoffTaskRow): string {
	switch (task.current_kind) {
		case 'observed':
			return 'Observed';
		case 'demonstrated':
			return 'Demonstrated';
		case 'revoked':
			return 'Revoked';
		case null:
			return 'Not signed off';
	}
}

export interface SignoffTask {
	task_id: number;
	competency_category: string;
	competency_name: string;
	prompt: string;
	kind: SignoffKind | null;
	reason: string | null;
	signed_by_display_name: string | null;
	signed_at: number | null;
	history: number;
}

export function signoffMatrix(
	enrollmentId: number
): Promise<{ tasks: SignoffTask[] }> {
	return request(`/api/enrollments/${enrollmentId}/signoffs`);
}

export function recordSignoff(
	enrollmentId: number,
	taskId: number,
	kind: SignoffKind,
	reason?: string
): Promise<void> {
	return request(`/api/enrollments/${enrollmentId}/signoffs`, {
		method: 'POST',
		body: JSON.stringify({ task_id: taskId, kind, reason })
	});
}
