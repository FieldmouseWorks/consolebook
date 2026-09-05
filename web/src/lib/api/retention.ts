import { request } from './transport';

export type RecordClass = 'daily_report' | 'weekly_summary' | 'phase_evaluation' | 'disposition_event';
export type RetentionTrigger = 'finalized_at' | 'enrollment_closed_at' | 'disposed_at';
export type RetentionAction = 'retain' | 'destroy';
export type HoldKind = 'litigation' | 'anticipated_litigation' | 'audit' | 'investigation' | 'public_records_request' | 'other';
export type HoldScope = { kind: 'installation' } | { kind: 'enrollment'; enrollment_id: number } | { kind: 'record'; record_id: number };
export interface PolicyInput {
    record_class: RecordClass; expected_current_id: number | null; authority: string;
    retention_trigger: RetentionTrigger; retention_days: number; action: RetentionAction; reason: string;
}
export interface Policy extends Omit<PolicyInput, 'expected_current_id'> {
    id: number; version_number: number; supersedes_id: number | null; created_by: number; created_at: number;
}
export interface HoldInput { scope: HoldScope; kind: HoldKind; authority: string; reason: string }
export interface Hold extends HoldInput {
    id: number; created_by: number; created_at: number; replaces_id: number | null;
    release: { released_by: number; released_at: number; reason: string; replacement_id: number | null } | null;
}
export interface AuthorityEvent { id: number; user_id: number; granted: boolean; actor_user_id: number; reason: string; recorded_at: number }
export interface ScopeOption { id: number; label: string }
export interface ScopeOptions { enrollments: ScopeOption[]; records: ScopeOption[] }
export const listPolicies = () => request<Policy[]>('/api/retention/policies');
export const savePolicy = (input: PolicyInput) => request<{ id: number }>('/api/retention/policies', { method: 'POST', body: JSON.stringify(input) });
export const listHolds = () => request<Hold[]>('/api/retention/holds');
export const listScopes = () => request<ScopeOptions>('/api/retention/scopes');
export const recordHolds = (id: number) => request<Hold[]>(`/api/retention/records/${id}/holds`);
export const saveHold = (input: HoldInput, replaces: number | null) => request<{ id: number }>(replaces === null ? '/api/retention/holds' : `/api/retention/holds/${replaces}/replace`, { method: 'POST', body: JSON.stringify(input) });
export const releaseHold = (id: number, reason: string) => request<void>(`/api/retention/holds/${id}/release`, { method: 'POST', body: JSON.stringify({ reason }) });
export const authorityHistory = () => request<AuthorityEvent[]>('/api/retention/authority');
export const setAuthority = (user_id: number, granted: boolean, reason: string) => request<void>('/api/retention/authority', { method: 'POST', body: JSON.stringify({ user_id, granted, reason }) });
export const classLabels: Record<RecordClass, string> = { daily_report: 'Daily reports', weekly_summary: 'Weekly summaries', phase_evaluation: 'Phase evaluations', disposition_event: 'Disposition events' };
export const triggerLabels: Record<RetentionTrigger, string> = { finalized_at: 'Finalization', enrollment_closed_at: 'Enrollment closure', disposed_at: 'Disposition' };
export const holdLabels: Record<HoldKind, string> = { litigation: 'Litigation', anticipated_litigation: 'Anticipated litigation', audit: 'Audit', investigation: 'Investigation', public_records_request: 'Public records request', other: 'Other authority' };
export function scopeLabel(scope: HoldScope, options: ScopeOptions): string {
    if (scope.kind === 'installation') return 'Entire installation';
    if (scope.kind === 'enrollment') return options.enrollments.find(e => e.id === scope.enrollment_id)?.label ?? `Enrollment ${scope.enrollment_id}`;
    return options.records.find(r => r.id === scope.record_id)?.label ?? `Record ${scope.record_id}`;
}
