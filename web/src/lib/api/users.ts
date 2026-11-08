import { request } from './transport';

// Users and enrollment (Milestone 3 slice 1: role bundles and profile
// fields at creation; full user administration is a later milestone).

export type Role = 'administrator' | 'coordinator' | 'trainer' | 'trainee';

export interface UserSummary {
	id: number;
	username: string;
	display_name: string;
	employee_id: string;
	title: string;
	created_at: number;
	capabilities: string[];
}

export function listUsers(): Promise<{ users: UserSummary[] }> {
	return request('/api/users');
}

export interface CreatedUser {
	id: number;
	username: string;
	display_name: string;
	reset_code: string;
	reset_expires_at: number;
}

export function createUser(input: {
	username: string;
	display_name: string;
	employee_id: string;
	title: string;
	role: Role;
}): Promise<CreatedUser> {
	return request('/api/users', { method: 'POST', body: JSON.stringify(input) });
}
