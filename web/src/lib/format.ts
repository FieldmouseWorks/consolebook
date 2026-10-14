// Presentation helpers shared across pages.

/** Renders a UTC unix-seconds instant in the viewer's locale. */
export function instant(unixSeconds: number): string {
	return new Date(unixSeconds * 1000).toLocaleString();
}

/**
 * A session's agency-local span as a date and a time range. The stored
 * strings are agency-local wall-clock values (`YYYY-MM-DDTHH:MM`), shown
 * as recorded; a span ending on a later date names that date.
 */
export function localSpan(start: string, end: string | null): { date: string; times: string } {
	const [startDate, startTime = ''] = start.split('T');
	if (end === null) {
		return { date: startDate, times: `from ${startTime}` };
	}
	const [endDate, endTime = ''] = end.split('T');
	const until = endDate === startDate ? endTime : `${endDate} ${endTime}`;
	return { date: startDate, times: `${startTime}–${until}` };
}

/** Plain names for the capability identifiers the server grants. */
const CAPABILITY_NAMES: Record<string, string> = {
	manage_users: 'Manage users',
	manage_programs: 'Manage programs',
	manage_retention: 'Manage retention',
	assign_training: 'Assign training',
	export_records: 'Export records',
	author_evaluation: 'Write evaluations',
	review_evaluation: 'Review evaluations',
	view_assigned_records: "View assigned trainees' records",
	view_own_records: 'View own records',
	acknowledge_own_record: 'Acknowledge own records'
};

/** Renders a capability for people; an unknown one shows its identifier. */
export function capabilityName(capability: string): string {
	return CAPABILITY_NAMES[capability] ?? capability;
}
