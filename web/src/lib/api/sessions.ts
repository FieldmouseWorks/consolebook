import { request } from './transport';

// Training sessions (Milestone 3 slice 2; ADR 0009): the entered local
// representation is stored verbatim, UTC is resolved server-side.

export type SessionDisposition = 'completed' | 'interrupted' | 'cancelled';

export interface SessionTrainer {
	user_id: number;
	username: string;
	display_name: string;
	added_at: number;
}

export interface TrainingSession {
	id: number;
	enrollment_id: number;
	business_date: string;
	timezone: string;
	local_start: string;
	local_end: string | null;
	utc_start: number;
	utc_end: number | null;
	phase_id: number | null;
	phase_name: string | null;
	disposition: SessionDisposition | null;
	created_at: number;
	created_by: number | null;
	closed_at: number | null;
	closed_by: number | null;
	draft_id: number | null;
	trainers: SessionTrainer[];
}

export interface MySession {
	session_id: number;
	enrollment_id: number;
	business_date: string;
	timezone: string;
	local_start: string;
	local_end: string | null;
	utc_start: number;
	disposition: SessionDisposition | null;
	phase_name: string | null;
	trainee_user_id: number;
	trainee_username: string;
	trainee_display_name: string;
	program_name: string;
	version_number: number;
	draft_id: number | null;
}

export function listSessions(
	enrollmentId: number
): Promise<{ sessions: TrainingSession[] }> {
	return request(`/api/enrollments/${enrollmentId}/sessions`);
}

export function createSession(
	enrollmentId: number,
	input: {
		business_date: string;
		timezone: string;
		local_start: string;
		local_end?: string;
		disposition?: SessionDisposition;
		phase_id?: number;
		trainer_user_ids: number[];
	}
): Promise<{ id: number }> {
	return request(`/api/enrollments/${enrollmentId}/sessions`, {
		method: 'POST',
		body: JSON.stringify(input)
	});
}

export function updateSession(
	sessionId: number,
	input: {
		business_date: string;
		timezone: string;
		local_start: string;
		phase_id?: number;
	}
): Promise<void> {
	return request(`/api/sessions/${sessionId}`, {
		method: 'PUT',
		body: JSON.stringify(input)
	});
}

export function closeSession(
	sessionId: number,
	disposition: SessionDisposition,
	localEnd?: string
): Promise<void> {
	return request(`/api/sessions/${sessionId}/close`, {
		method: 'POST',
		body: JSON.stringify({ disposition, local_end: localEnd })
	});
}

export function addSessionTrainer(
	sessionId: number,
	trainerUserId: number
): Promise<void> {
	return request(`/api/sessions/${sessionId}/trainers`, {
		method: 'POST',
		body: JSON.stringify({ trainer_user_id: trainerUserId })
	});
}

export function removeSessionTrainer(sessionId: number, userId: number): Promise<void> {
	return request(`/api/sessions/${sessionId}/trainers/${userId}`, { method: 'DELETE' });
}

export function mySessions(): Promise<{ sessions: MySession[] }> {
	return request('/api/sessions/mine');
}
