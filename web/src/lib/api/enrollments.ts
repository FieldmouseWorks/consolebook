import { request } from './transport';
import type { TransitionKind } from './programs';

export interface Enrollee {
	enrollment_id: number;
	user_id: number;
	username: string;
	display_name: string;
	enrolled_at: number;
	enrolled_by: number | null;
}

export function listEnrollments(versionId: number): Promise<{ enrollees: Enrollee[] }> {
	return request(`/api/program-versions/${versionId}/enrollments`);
}

export function enrollUser(versionId: number, userId: number): Promise<{ id: number }> {
	return request(`/api/program-versions/${versionId}/enrollments`, {
		method: 'POST',
		body: JSON.stringify({ user_id: userId })
	});
}

// Training lifecycle (Milestone 3 slice 1: assignments, enrollment
// lifecycle events, and phase history; field names mirror the server).

export type EnrollmentStatus = 'active' | 'withdrawn' | 'completed';
export type EnrollmentEventKind = 'version_change' | 'withdraw' | 'complete' | 'reinstate';
export type PhaseEventKind = 'advance' | 'return' | 'restart' | 'pause' | 'resume' | 'complete';

export interface EnrollmentEvent {
	id: number;
	kind: string;
	occurred_at: number;
	actor_user_id: number | null;
	actor_display_name: string | null;
	reason: string;
	from_program_version_id: number | null;
	from_version_number: number | null;
	from_version_label: string | null;
	to_program_version_id: number | null;
	to_version_number: number | null;
	to_version_label: string | null;
}

export interface PhaseEvent {
	id: number;
	kind: string;
	from_phase_id: number | null;
	from_phase_name: string | null;
	to_phase_id: number | null;
	to_phase_name: string | null;
	effective_at: number;
	recorded_at: number;
	actor_user_id: number | null;
	actor_display_name: string | null;
	reason: string;
}

export interface PhaseRef {
	id: number;
	name: string;
	presentation_number: number;
}

export interface TransitionRef {
	from_phase_id: number;
	to_phase_id: number;
	kind: TransitionKind;
}

export interface Assignment {
	id: number;
	enrollment_id: number;
	trainer_user_id: number;
	trainer_username: string;
	trainer_display_name: string;
	assigned_at: number;
	assigned_by: number | null;
	ended_at: number | null;
	ended_by: number | null;
}

export interface AssignedTrainee {
	assignment_id: number;
	enrollment_id: number;
	trainee_user_id: number;
	trainee_username: string;
	trainee_display_name: string;
	program_version_id: number;
	program_name: string;
	version_number: number;
	version_label: string;
	assigned_at: number;
}

export interface EnrollmentDetail {
	enrollment_id: number;
	trainee_user_id: number;
	trainee_username: string;
	trainee_display_name: string;
	enrolled_at: number;
	program_id: number;
	program_version_id: number;
	program_name: string;
	version_number: number;
	version_label: string;
	status: EnrollmentStatus;
	paused: boolean;
	current_phase_id: number | null;
	current_phase_name: string | null;
	events: EnrollmentEvent[];
	phase_events: PhaseEvent[];
	assignments: Assignment[];
	phases: PhaseRef[];
	transitions: TransitionRef[];
}

export function getEnrollment(enrollmentId: number): Promise<EnrollmentDetail> {
	return request(`/api/enrollments/${enrollmentId}`);
}

export function recordEnrollmentEvent(
	enrollmentId: number,
	input: { kind: EnrollmentEventKind; reason: string; to_version_id?: number }
): Promise<{ id: number }> {
	return request(`/api/enrollments/${enrollmentId}/events`, {
		method: 'POST',
		body: JSON.stringify(input)
	});
}

export function recordPhaseEvent(
	enrollmentId: number,
	input: {
		kind: PhaseEventKind;
		to_phase_id?: number;
		effective_at?: number;
		reason: string;
	}
): Promise<{ id: number }> {
	return request(`/api/enrollments/${enrollmentId}/phase-events`, {
		method: 'POST',
		body: JSON.stringify(input)
	});
}

export function createAssignment(
	enrollmentId: number,
	trainerUserId: number
): Promise<{ id: number }> {
	return request(`/api/enrollments/${enrollmentId}/assignments`, {
		method: 'POST',
		body: JSON.stringify({ trainer_user_id: trainerUserId })
	});
}

export function endAssignment(assignmentId: number): Promise<void> {
	return request(`/api/assignments/${assignmentId}/end`, {
		method: 'POST',
		body: JSON.stringify({})
	});
}

export function myAssignments(): Promise<{ assignments: AssignedTrainee[] }> {
	return request('/api/assignments/mine');
}

// Trainee packets (Milestone 5 slice 2; docs/formats/trainee-packet.md;
// ADR 0015): everything retained about one enrollment as one archive.

export interface OwnEnrollment {
	enrollment_id: number;
	program_name: string;
	version_number: number;
	version_label: string;
	enrolled_at: number;
	status: EnrollmentStatus;
	finalized_versions: number;
}

export function myEnrollments(): Promise<{ enrollments: OwnEnrollment[] }> {
	return request('/api/my/enrollments');
}

/** Download path for an enrollment's trainee packet. */
export function enrollmentPacketPath(enrollmentId: number): string {
	return `/api/enrollments/${enrollmentId}/packet`;
}
