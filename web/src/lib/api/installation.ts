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

export type DispositionRecoveryState =
	| 'clear'
	| 'incomplete_disposition'
	| 'ledger_error'
	| 'ledger_stale'
	| 'copy_ledger_unavailable'
	| 'copy_ledger_behind';

export interface IncompleteDispositionRun {
	id: number;
	state: 'db_committed_vacuum_pending' | 'copy_cleanup_pending' | 'completed';
	started_at: number;
	last_error_code: string | null;
	event_count: number;
}

export type LocalLedgerStatus =
	| { state: 'absent' }
	| { state: 'present'; entry_count: number; updated_at: number }
	| { state: 'stale'; missing_count: number }
	| { state: 'corrupt'; problem: string };

export type CopyLedgerStatus =
	| { state: 'not_configured' }
	| { state: 'not_required'; directory: string }
	| { state: 'unreachable'; directory: string; problem: string }
	| { state: 'missing'; directory: string }
	| { state: 'behind'; directory: string; local_count: number; copy_count: number }
	| { state: 'current'; directory: string; entry_count: number }
	| { state: 'corrupt'; directory: string; problem: string };

export interface DispositionRecoveryHealth {
	state: DispositionRecoveryState;
	incomplete_runs: IncompleteDispositionRun[];
	local_ledger: LocalLedgerStatus;
	copy_ledger: CopyLedgerStatus;
	external_copies_note: string;
}

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
	disposition_recovery: DispositionRecoveryHealth;
}

export function getBackupStatus(): Promise<BackupStatus> {
	return request('/api/backups/status');
}

/** The installer command that adds an off-server copy directory. */
export const BACKUP_DIR_COMMAND =
	'curl -fsSL https://github.com/FieldmouseWorks/consolebook/releases/latest/download/install.sh | sudo sh -s -- --backup-dir /path/to/second/disk';
