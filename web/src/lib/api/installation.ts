import { request } from './transport';

/** Installation-wide counts behind the administrator's setup checklist. */
export interface SetupProgress {
	coordinators: number;
	trainers: number;
	trainees: number;
	programs: number;
	published_versions: number;
	enrollments: number;
	assignments: number;
	sessions: number;
	finalized_records: number;
}

export function getSetupProgress(): Promise<SetupProgress> {
	return request('/api/getting-started');
}

/** Where off-server backup copies stand (ADR 0024). */
export type BackupCopyStatus =
	| { state: 'not_configured' }
	| { state: 'unreachable'; directory: string; problem: string }
	| { state: 'behind'; directory: string; count: number; newest: string | null }
	| { state: 'current'; directory: string; count: number; newest: string };

export interface BackupStatus {
	interval_hours: number;
	keep: number;
	local: {
		directory: string;
		count: number;
		newest: string | null;
		newest_at: number | null;
	};
	copy: BackupCopyStatus;
}

export function getBackupStatus(): Promise<BackupStatus> {
	return request('/api/backups/status');
}

/** The installer command that adds an off-server copy directory. */
export const BACKUP_DIR_COMMAND =
	'curl -fsSL https://github.com/FieldmouseWorks/consolebook/releases/latest/download/install.sh | sudo sh -s -- --backup-dir /path/to/second/disk';
