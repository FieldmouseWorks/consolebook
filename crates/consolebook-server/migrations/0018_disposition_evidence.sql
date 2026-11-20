-- ADR 0030; #212: minimal disposition event storage, run state machine, and
-- policy-required metadata boundary checkpoints. No destruction execution or
-- metadata expiry path is enabled here.
--
-- None of these tables carries record narratives, ratings, acknowledgments,
-- display-name snapshots, or free-form reasons. Every text field is either a
-- closed vocabulary code, a 64-character lowercase SHA-256 hex digest, or a
-- bounded agency authority reference.

CREATE TABLE disposition_run (
    id INTEGER PRIMARY KEY,
    actor_user_id INTEGER NOT NULL REFERENCES user(id),
    confirmation_digest TEXT NOT NULL
        CHECK (length(confirmation_digest) = 64 AND confirmation_digest NOT GLOB '*[^0-9a-f]*'),
    authority_reference TEXT NOT NULL
        CHECK (length(trim(authority_reference, char(9, 10, 11, 12, 13, 32, 133, 160, 5760, 8192, 8193, 8194, 8195, 8196, 8197, 8198, 8199, 8200, 8201, 8202, 8232, 8233, 8239, 8287, 12288))) BETWEEN 1 AND 200),
    reason_code TEXT NOT NULL
        CHECK (reason_code IN ('scheduled_retention_elapsed', 'restore_ledger_reconciliation', 'metadata_retention_elapsed')),
    method TEXT NOT NULL
        CHECK (method IN ('lawful_destruction', 'restore_reconciliation')),
    state TEXT NOT NULL
        CHECK (state IN ('db_committed_vacuum_pending', 'copy_cleanup_pending', 'completed')),
    started_at INTEGER NOT NULL,
    completed_at INTEGER,
    last_error_code TEXT
        CHECK (last_error_code IS NULL OR last_error_code IN (
            'vacuum_failed',
            'ledger_write_failed',
            'clean_snapshot_failed',
            'local_cleanup_failed',
            'copy_dir_unreachable',
            'copy_cleanup_failed'
        )),
    CHECK ((state = 'completed') = (completed_at IS NOT NULL)),
    CHECK (completed_at IS NULL OR completed_at >= started_at),
    CHECK (state != 'completed' OR last_error_code IS NULL)
) STRICT;

CREATE TRIGGER disposition_run_no_delete
BEFORE DELETE ON disposition_run
BEGIN
    SELECT RAISE(ABORT, 'disposition runs are permanent accountability records');
END;

CREATE TRIGGER disposition_run_immutable_identity
BEFORE UPDATE ON disposition_run
WHEN OLD.state = 'completed'
    OR NEW.id != OLD.id
    OR NEW.actor_user_id != OLD.actor_user_id
    OR NEW.confirmation_digest != OLD.confirmation_digest
    OR NEW.authority_reference != OLD.authority_reference
    OR NEW.reason_code != OLD.reason_code
    OR NEW.method != OLD.method
    OR NEW.started_at != OLD.started_at
    OR (OLD.state = 'copy_cleanup_pending' AND NEW.state = 'db_committed_vacuum_pending')
BEGIN
    SELECT RAISE(ABORT, 'disposition run state advances monotonically and identity is immutable');
END;

-- Minimal tombstone for a disposed evaluation record lineage. Deliberately has
-- no foreign key to evaluation_record so the disposed evaluation_record row
-- can be removed while retaining opaque provenance.
CREATE TABLE disposition_event (
    id INTEGER PRIMARY KEY,
    disposition_run_id INTEGER NOT NULL REFERENCES disposition_run(id),
    evaluation_record_id INTEGER NOT NULL UNIQUE,
    enrollment_id INTEGER NOT NULL REFERENCES enrollment(id),
    record_class TEXT NOT NULL
        CHECK (record_class IN ('daily_report', 'weekly_summary', 'phase_evaluation')),
    record_policy_id INTEGER NOT NULL REFERENCES retention_policy(id),
    metadata_policy_id INTEGER REFERENCES retention_policy(id),
    version_count INTEGER NOT NULL CHECK (version_count > 0),
    earliest_finalized_at INTEGER NOT NULL,
    latest_finalized_at INTEGER NOT NULL CHECK (latest_finalized_at >= earliest_finalized_at),
    hashes_retained INTEGER NOT NULL CHECK (hashes_retained IN (0, 1)),
    actor_user_id INTEGER NOT NULL REFERENCES user(id),
    authority_reference TEXT NOT NULL
        CHECK (length(trim(authority_reference, char(9, 10, 11, 12, 13, 32, 133, 160, 5760, 8192, 8193, 8194, 8195, 8196, 8197, 8198, 8199, 8200, 8201, 8202, 8232, 8233, 8239, 8287, 12288))) BETWEEN 1 AND 200),
    reason_code TEXT NOT NULL
        CHECK (reason_code IN ('scheduled_retention_elapsed', 'restore_ledger_reconciliation')),
    method TEXT NOT NULL
        CHECK (method IN ('lawful_destruction', 'restore_reconciliation')),
    outcome TEXT NOT NULL CHECK (outcome IN ('disposed')),
    disposed_at INTEGER NOT NULL CHECK (disposed_at >= latest_finalized_at)
) STRICT;

CREATE INDEX disposition_event_run ON disposition_event(disposition_run_id);
CREATE INDEX disposition_event_enrollment ON disposition_event(enrollment_id);

CREATE TRIGGER disposition_event_policy_matches
BEFORE INSERT ON disposition_event
WHEN NOT EXISTS (
        SELECT 1 FROM retention_policy p
        WHERE p.id = NEW.record_policy_id
          AND p.record_class = NEW.record_class
          AND p.action = 'destroy'
    )
    OR (
        NEW.metadata_policy_id IS NOT NULL
        AND NOT EXISTS (
            SELECT 1 FROM retention_policy p
            WHERE p.id = NEW.metadata_policy_id
              AND p.record_class = 'disposition_event'
        )
    )
BEGIN
    SELECT RAISE(ABORT, 'disposition event must reference matching destroy record policy and disposition_event metadata policy');
END;

CREATE TRIGGER disposition_event_no_update
BEFORE UPDATE ON disposition_event
BEGIN
    SELECT RAISE(ABORT, 'disposition events are immutable');
END;

CREATE TRIGGER disposition_event_no_delete
BEFORE DELETE ON disposition_event
BEGIN
    SELECT RAISE(ABORT, 'disposition events cannot be deleted directly');
END;

-- Optional retained version fingerprints for lineage continuity while
-- permitted by the disposition_event metadata policy.
CREATE TABLE disposition_version_hash (
    id INTEGER PRIMARY KEY,
    disposition_event_id INTEGER NOT NULL REFERENCES disposition_event(id),
    version_number INTEGER NOT NULL CHECK (version_number > 0),
    record_schema INTEGER NOT NULL CHECK (record_schema > 0),
    content_hash TEXT NOT NULL
        CHECK (length(content_hash) = 64 AND content_hash NOT GLOB '*[^0-9a-f]*'),
    chain_hash TEXT NOT NULL
        CHECK (length(chain_hash) = 64 AND chain_hash NOT GLOB '*[^0-9a-f]*'),
    predecessor_content_hash TEXT
        CHECK (predecessor_content_hash IS NULL OR (length(predecessor_content_hash) = 64 AND predecessor_content_hash NOT GLOB '*[^0-9a-f]*')),
    finalized_at INTEGER NOT NULL,
    UNIQUE (disposition_event_id, version_number),
    CHECK ((version_number = 1) = (predecessor_content_hash IS NULL))
) STRICT;

CREATE INDEX disposition_version_hash_event ON disposition_version_hash(disposition_event_id);

CREATE TRIGGER disposition_version_hash_matches_event
BEFORE INSERT ON disposition_version_hash
WHEN NOT EXISTS (
        SELECT 1 FROM disposition_event e
        WHERE e.id = NEW.disposition_event_id
          AND e.hashes_retained = 1
          AND NEW.version_number <= e.version_count
    )
BEGIN
    SELECT RAISE(ABORT, 'version hash requires a hash-retaining disposition event within version_count');
END;

CREATE TRIGGER disposition_version_hash_no_update
BEFORE UPDATE ON disposition_version_hash
BEGIN
    SELECT RAISE(ABORT, 'disposition version hashes are immutable');
END;

CREATE TRIGGER disposition_version_hash_no_delete
BEFORE DELETE ON disposition_version_hash
BEGIN
    SELECT RAISE(ABORT, 'disposition version hashes cannot be deleted directly');
END;

-- Non-personal boundary marker left behind when expired disposition_event
-- tombstones are removed under a disposition_event destroy policy (#218).
CREATE TABLE disposition_boundary_checkpoint (
    id INTEGER PRIMARY KEY,
    disposition_run_id INTEGER NOT NULL REFERENCES disposition_run(id),
    metadata_policy_id INTEGER NOT NULL REFERENCES retention_policy(id),
    authority_reference TEXT NOT NULL
        CHECK (length(trim(authority_reference, char(9, 10, 11, 12, 13, 32, 133, 160, 5760, 8192, 8193, 8194, 8195, 8196, 8197, 8198, 8199, 8200, 8201, 8202, 8232, 8233, 8239, 8287, 12288))) BETWEEN 1 AND 200),
    reason_code TEXT NOT NULL
        CHECK (reason_code IN ('metadata_retention_elapsed')),
    expired_event_count INTEGER NOT NULL CHECK (expired_event_count > 0),
    boundary_through_disposed_at INTEGER NOT NULL,
    recorded_by INTEGER NOT NULL REFERENCES user(id),
    recorded_at INTEGER NOT NULL CHECK (recorded_at >= boundary_through_disposed_at)
) STRICT;

CREATE INDEX disposition_boundary_checkpoint_run
    ON disposition_boundary_checkpoint(disposition_run_id);

CREATE TRIGGER disposition_boundary_checkpoint_policy_matches
BEFORE INSERT ON disposition_boundary_checkpoint
WHEN NOT EXISTS (
        SELECT 1 FROM retention_policy p
        WHERE p.id = NEW.metadata_policy_id
          AND p.record_class = 'disposition_event'
          AND p.action = 'destroy'
    )
BEGIN
    SELECT RAISE(ABORT, 'boundary checkpoint must reference a destroy disposition_event policy');
END;

CREATE TRIGGER disposition_boundary_checkpoint_no_update
BEFORE UPDATE ON disposition_boundary_checkpoint
BEGIN
    SELECT RAISE(ABORT, 'disposition boundary checkpoints are immutable');
END;

CREATE TRIGGER disposition_boundary_checkpoint_no_delete
BEFORE DELETE ON disposition_boundary_checkpoint
BEGIN
    SELECT RAISE(ABORT, 'disposition boundary checkpoints cannot be deleted');
END;
