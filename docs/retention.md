# Retention administration

Consolebook currently supports versioned retention policy configuration and
attributed record holds. **Disposition execution is not available yet.** Saving
a policy, releasing a hold, or checking holds does not delete records or prove
that destruction is authorized. The remaining workflow is tracked in
[#64](https://github.com/FieldmouseWorks/consolebook/issues/64).

## Assign an operator

A user with `manage_users` opens **Retention → Retention authority**, selects
an existing user, records a reason, and explicitly grants retention
administration. No role receives this permission automatically. The same
control revokes it and preserves the attribution history. The permission
allows installation-wide policy and hold metadata access; grant it accordingly.
It does not grant destruction authority.

## Record the approved schedule

A retention administrator selects the record class, enters the agency's
approved authority reference, chooses the trigger and action, and records
why the version is being adopted. Consolebook supplies no default schedule.

- Evaluation policies support finalization or enrollment closure as triggers.
- Disposition-event policies use disposition time, independently of evaluation
  policy. Their execution and event expiry arrive with the disposition work.
- Periods are elapsed 24-hour days, not calendar-month/year arithmetic.
- **Retain** authorizes no destruction; **Destroy** records schedule intent
  subject to holds and the future confirmed-disposition workflow.

Saving creates an immutable version. Revisions preserve earlier versions. If
someone else changes the current policy while it is being edited, the save
refuses with a conflict. Use **Load current policy**, review its values, and
enter a new reason before submitting again. Loading current values replaces
unsaved policy fields.

## Place, change, and release holds

Choose the entire installation, a named enrollment, or a named record as the
scope. Choose the hold kind, enter its authority reference, and explain the
preservation instruction. Use invented data in development; authority and
reason fields in a real installation should contain only the metadata needed
to administer the hold, not copied record narratives.

Holds never expire automatically. **Replace hold** records a new scope/kind
and reason and releases the previous hold in the same operation; if replacement
fails, the old hold stays active. **Release hold** displays the scope again and
requires a reason and explicit confirmation. Prior holds and releases remain
visible in history with actor ids and timestamps.

**Check a record's holds** lists the active installation, enrollment, and
record holds that apply to it. A result of zero is not a disposition approval:
policy eligibility, related records and copies, authority, and exact-scope
confirmation have not been evaluated by this lookup.

## Remaining disposition work

Finalized records retain their existing immutability guards. There is no
supported command or web control to bypass them. The next stage must define
and prove the complete destruction scope, including duplicate content,
summary dependencies, backups and restore, failure/retry handling, minimal
tombstones with independent retention, and honest export verification of
unavailable records and policy boundaries. See [ADR 0020](decisions/0020-retention-policy-and-hold-administration.md).
