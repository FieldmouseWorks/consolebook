-- ADR 0020; #65: retention administration, without a destruction path.
CREATE TABLE retention_authority_event (
    id INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES user(id),
    granted INTEGER NOT NULL CHECK (granted IN (0, 1)),
    actor_user_id INTEGER NOT NULL REFERENCES user(id),
    reason TEXT NOT NULL CHECK (length(trim(reason, char(9, 10, 11, 12, 13, 32, 133, 160, 5760, 8192, 8193, 8194, 8195, 8196, 8197, 8198, 8199, 8200, 8201, 8202, 8232, 8233, 8239, 8287, 12288))) BETWEEN 1 AND 1000),
    recorded_at INTEGER NOT NULL
) STRICT;

CREATE TABLE retention_policy (
    id INTEGER PRIMARY KEY,
    record_class TEXT NOT NULL CHECK (record_class IN ('daily_report', 'weekly_summary', 'phase_evaluation', 'disposition_event')),
    version_number INTEGER NOT NULL CHECK (version_number > 0),
    supersedes_id INTEGER UNIQUE REFERENCES retention_policy(id),
    authority TEXT NOT NULL CHECK (length(trim(authority, char(9, 10, 11, 12, 13, 32, 133, 160, 5760, 8192, 8193, 8194, 8195, 8196, 8197, 8198, 8199, 8200, 8201, 8202, 8232, 8233, 8239, 8287, 12288))) BETWEEN 1 AND 200),
    retention_trigger TEXT NOT NULL CHECK (retention_trigger IN ('finalized_at', 'enrollment_closed_at', 'disposed_at')),
    retention_days INTEGER NOT NULL CHECK (retention_days BETWEEN 0 AND 365250),
    action TEXT NOT NULL CHECK (action IN ('retain', 'destroy')),
    reason TEXT NOT NULL CHECK (length(trim(reason, char(9, 10, 11, 12, 13, 32, 133, 160, 5760, 8192, 8193, 8194, 8195, 8196, 8197, 8198, 8199, 8200, 8201, 8202, 8232, 8233, 8239, 8287, 12288))) BETWEEN 1 AND 1000),
    created_by INTEGER NOT NULL REFERENCES user(id),
    created_at INTEGER NOT NULL,
    UNIQUE(record_class, version_number),
    CHECK ((record_class = 'disposition_event') = (retention_trigger = 'disposed_at')),
    CHECK (action = 'destroy' OR retention_days = 0),
    CHECK ((version_number = 1) = (supersedes_id IS NULL))
) STRICT;

CREATE TRIGGER retention_policy_successor
BEFORE INSERT ON retention_policy
WHEN NEW.version_number != (SELECT COALESCE(MAX(version_number), 0) + 1 FROM retention_policy WHERE record_class = NEW.record_class)
    OR NEW.supersedes_id IS NOT (SELECT id FROM retention_policy WHERE record_class = NEW.record_class ORDER BY version_number DESC LIMIT 1)
BEGIN
    SELECT RAISE(ABORT, 'a retention policy supersedes the current version of its class');
END;

CREATE TABLE record_hold (
    id INTEGER PRIMARY KEY,
    enrollment_id INTEGER REFERENCES enrollment(id),
    evaluation_record_id INTEGER REFERENCES evaluation_record(id),
    -- No ids means installation; enrollment only means enrollment scope;
    -- record only means that record. No implicit name or text matching.
    kind TEXT NOT NULL CHECK (kind IN ('litigation', 'anticipated_litigation', 'audit', 'investigation', 'public_records_request', 'other')),
    authority TEXT NOT NULL CHECK (length(trim(authority, char(9, 10, 11, 12, 13, 32, 133, 160, 5760, 8192, 8193, 8194, 8195, 8196, 8197, 8198, 8199, 8200, 8201, 8202, 8232, 8233, 8239, 8287, 12288))) BETWEEN 1 AND 200),
    reason TEXT NOT NULL CHECK (length(trim(reason, char(9, 10, 11, 12, 13, 32, 133, 160, 5760, 8192, 8193, 8194, 8195, 8196, 8197, 8198, 8199, 8200, 8201, 8202, 8232, 8233, 8239, 8287, 12288))) BETWEEN 1 AND 1000),
    created_by INTEGER NOT NULL REFERENCES user(id),
    created_at INTEGER NOT NULL,
    replaces_id INTEGER UNIQUE REFERENCES record_hold(id),
    CHECK (enrollment_id IS NULL OR evaluation_record_id IS NULL)
) STRICT;

CREATE TABLE hold_release (
    hold_id INTEGER PRIMARY KEY REFERENCES record_hold(id),
    released_by INTEGER NOT NULL REFERENCES user(id),
    released_at INTEGER NOT NULL,
    reason TEXT NOT NULL CHECK (length(trim(reason, char(9, 10, 11, 12, 13, 32, 133, 160, 5760, 8192, 8193, 8194, 8195, 8196, 8197, 8198, 8199, 8200, 8201, 8202, 8232, 8233, 8239, 8287, 12288))) BETWEEN 1 AND 1000),
    replacement_id INTEGER UNIQUE REFERENCES record_hold(id)
) STRICT;

CREATE TRIGGER record_hold_replace_active
BEFORE INSERT ON record_hold
WHEN NEW.replaces_id IS NOT NULL AND (
    NOT EXISTS (SELECT 1 FROM record_hold WHERE id = NEW.replaces_id)
    OR EXISTS (SELECT 1 FROM hold_release WHERE hold_id = NEW.replaces_id)
    OR NEW.created_at < (SELECT created_at FROM record_hold WHERE id = NEW.replaces_id)
)
BEGIN
    SELECT RAISE(ABORT, 'a released hold cannot be replaced');
END;

CREATE TRIGGER hold_release_chronology
BEFORE INSERT ON hold_release
WHEN NEW.released_at < (SELECT created_at FROM record_hold WHERE id = NEW.hold_id)
BEGIN
    SELECT RAISE(ABORT, 'a hold release cannot precede its creation');
END;

CREATE TRIGGER hold_release_matches_replacement
BEFORE INSERT ON hold_release
WHEN NEW.replacement_id IS NOT NULL AND NOT EXISTS (
    SELECT 1 FROM record_hold h WHERE h.id = NEW.replacement_id AND h.replaces_id = NEW.hold_id
        AND h.created_by = NEW.released_by AND h.created_at = NEW.released_at AND h.reason = NEW.reason
)
BEGIN
    SELECT RAISE(ABORT, 'a replacement release names its attributed successor');
END;

CREATE TRIGGER record_hold_replace_releases_previous
AFTER INSERT ON record_hold
WHEN NEW.replaces_id IS NOT NULL
BEGIN
    INSERT INTO hold_release (hold_id, released_by, released_at, reason, replacement_id)
    VALUES (NEW.replaces_id, NEW.created_by, NEW.created_at, NEW.reason, NEW.id);
END;

CREATE INDEX record_hold_enrollment ON record_hold(enrollment_id);
CREATE INDEX record_hold_record ON record_hold(evaluation_record_id);

CREATE TRIGGER retention_authority_event_no_update
BEFORE UPDATE ON retention_authority_event
BEGIN
    SELECT RAISE(ABORT, 'retention administration history is append-only');
END;

CREATE TRIGGER retention_authority_event_no_delete
BEFORE DELETE ON retention_authority_event
BEGIN
    SELECT RAISE(ABORT, 'retention administration history is append-only');
END;

CREATE TRIGGER retention_policy_no_update
BEFORE UPDATE ON retention_policy
BEGIN
    SELECT RAISE(ABORT, 'retention administration history is append-only');
END;

CREATE TRIGGER retention_policy_no_delete
BEFORE DELETE ON retention_policy
BEGIN
    SELECT RAISE(ABORT, 'retention administration history is append-only');
END;

CREATE TRIGGER record_hold_no_update
BEFORE UPDATE ON record_hold
BEGIN
    SELECT RAISE(ABORT, 'retention administration history is append-only');
END;

CREATE TRIGGER record_hold_no_delete
BEFORE DELETE ON record_hold
BEGIN
    SELECT RAISE(ABORT, 'retention administration history is append-only');
END;

CREATE TRIGGER hold_release_no_update
BEFORE UPDATE ON hold_release
BEGIN
    SELECT RAISE(ABORT, 'retention administration history is append-only');
END;

CREATE TRIGGER hold_release_no_delete
BEFORE DELETE ON hold_release
BEGIN
    SELECT RAISE(ABORT, 'retention administration history is append-only');
END;
