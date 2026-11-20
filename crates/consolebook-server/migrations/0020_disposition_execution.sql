-- ADR 0030; #214: schema gates for lawful disposition execution.
--
-- 1. Decouples record_hold.evaluation_record_id's blocking foreign key so
--    released record-scoped holds remain append-only history without blocking
--    lawful deletion of evaluation_record, while an insert-time trigger still
--    requires the referenced record to exist at hold creation time.
-- 2. Adds nullable notice.evaluation_record_id so newly created evaluation
--    workflow notices are deleted with their disposed record while legacy
--    unlinked notices remain untouched and documented.
-- 3. Replaces in-scope *_no_delete / *_frozen_delete triggers (and adds
--    evaluation_record_no_delete) so raw ad-hoc DELETE continues to fail
--    closed outside an active, authorized disposition_run in
--    'db_committed_vacuum_pending' with a matching disposition_event row.

-- ---------------------------------------------------------------- 1. record_hold FK decoupling

CREATE TABLE record_hold_new (
    id INTEGER PRIMARY KEY,
    enrollment_id INTEGER REFERENCES enrollment(id),
    evaluation_record_id INTEGER,
    kind TEXT NOT NULL CHECK (kind IN ('litigation', 'anticipated_litigation', 'audit', 'investigation', 'public_records_request', 'other')),
    authority TEXT NOT NULL CHECK (length(trim(authority, char(9, 10, 11, 12, 13, 32, 133, 160, 5760, 8192, 8193, 8194, 8195, 8196, 8197, 8198, 8199, 8200, 8201, 8202, 8232, 8233, 8239, 8287, 12288))) BETWEEN 1 AND 200),
    reason TEXT NOT NULL CHECK (length(trim(reason, char(9, 10, 11, 12, 13, 32, 133, 160, 5760, 8192, 8193, 8194, 8195, 8196, 8197, 8198, 8199, 8200, 8201, 8202, 8232, 8233, 8239, 8287, 12288))) BETWEEN 1 AND 1000),
    created_by INTEGER NOT NULL REFERENCES user(id),
    created_at INTEGER NOT NULL,
    replaces_id INTEGER UNIQUE REFERENCES record_hold_new(id),
    CHECK (enrollment_id IS NULL OR evaluation_record_id IS NULL)
) STRICT;

CREATE TABLE hold_release_new (
    hold_id INTEGER PRIMARY KEY REFERENCES record_hold_new(id),
    released_by INTEGER NOT NULL REFERENCES user(id),
    released_at INTEGER NOT NULL,
    reason TEXT NOT NULL CHECK (length(trim(reason, char(9, 10, 11, 12, 13, 32, 133, 160, 5760, 8192, 8193, 8194, 8195, 8196, 8197, 8198, 8199, 8200, 8201, 8202, 8232, 8233, 8239, 8287, 12288))) BETWEEN 1 AND 1000),
    replacement_id INTEGER UNIQUE REFERENCES record_hold_new(id)
) STRICT;

INSERT INTO record_hold_new (id, enrollment_id, evaluation_record_id, kind, authority, reason, created_by, created_at, replaces_id)
SELECT id, enrollment_id, evaluation_record_id, kind, authority, reason, created_by, created_at, replaces_id
FROM record_hold
ORDER BY id ASC;

INSERT INTO hold_release_new (hold_id, released_by, released_at, reason, replacement_id)
SELECT hold_id, released_by, released_at, reason, replacement_id
FROM hold_release
ORDER BY hold_id ASC;

DROP TABLE hold_release;
DROP TABLE record_hold;

ALTER TABLE record_hold_new RENAME TO record_hold;
ALTER TABLE hold_release_new RENAME TO hold_release;

CREATE INDEX record_hold_enrollment ON record_hold(enrollment_id);
CREATE INDEX record_hold_record ON record_hold(evaluation_record_id);

CREATE TRIGGER record_hold_record_exists
BEFORE INSERT ON record_hold
WHEN NEW.evaluation_record_id IS NOT NULL
    AND NOT EXISTS (SELECT 1 FROM evaluation_record WHERE id = NEW.evaluation_record_id)
BEGIN
    SELECT RAISE(ABORT, 'record-scoped hold must reference an existing evaluation record');
END;

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

-- ---------------------------------------------------------------- 2. Record-linked notices

ALTER TABLE notice ADD COLUMN evaluation_record_id INTEGER;
CREATE INDEX notice_evaluation_record ON notice(evaluation_record_id);

CREATE TRIGGER notice_record_exists
BEFORE INSERT ON notice
WHEN NEW.evaluation_record_id IS NOT NULL
    AND NOT EXISTS (SELECT 1 FROM evaluation_record WHERE id = NEW.evaluation_record_id)
BEGIN
    SELECT RAISE(ABORT, 'record-linked notice must reference an existing evaluation record');
END;

-- ---------------------------------------------------------------- 3. Schema-gated disposition deletes

DROP TRIGGER evaluation_version_no_delete;
CREATE TRIGGER evaluation_version_no_delete
BEFORE DELETE ON evaluation_version
WHEN NOT EXISTS (
    SELECT 1
    FROM disposition_event de
    JOIN disposition_run dr ON dr.id = de.disposition_run_id
    WHERE de.evaluation_record_id = OLD.evaluation_record_id
      AND dr.state = 'db_committed_vacuum_pending'
)
BEGIN
    SELECT RAISE(ABORT, 'a finalized version is immutable while retained');
END;

DROP TRIGGER acknowledgment_no_delete;
CREATE TRIGGER acknowledgment_no_delete
BEFORE DELETE ON acknowledgment
WHEN NOT EXISTS (
    SELECT 1
    FROM disposition_event de
    JOIN disposition_run dr ON dr.id = de.disposition_run_id
    WHERE de.evaluation_record_id = (
        SELECT ev.evaluation_record_id FROM evaluation_version ev WHERE ev.id = OLD.evaluation_version_id
    )
      AND dr.state = 'db_committed_vacuum_pending'
)
BEGIN
    SELECT RAISE(ABORT, 'acknowledgments are permanent records');
END;

DROP TRIGGER amendment_no_delete;
CREATE TRIGGER amendment_no_delete
BEFORE DELETE ON amendment
WHEN NOT EXISTS (
    SELECT 1
    FROM disposition_event de
    JOIN disposition_run dr ON dr.id = de.disposition_run_id
    WHERE de.evaluation_record_id = OLD.evaluation_record_id
      AND dr.state = 'db_committed_vacuum_pending'
)
BEGIN
    SELECT RAISE(ABORT, 'amendments are permanent records');
END;

DROP TRIGGER draft_snapshot_no_delete;
CREATE TRIGGER draft_snapshot_no_delete
BEFORE DELETE ON draft_snapshot
WHEN NOT EXISTS (
    SELECT 1
    FROM disposition_event de
    JOIN disposition_run dr ON dr.id = de.disposition_run_id
    WHERE de.evaluation_record_id = OLD.evaluation_record_id
      AND dr.state = 'db_committed_vacuum_pending'
)
BEGIN
    SELECT RAISE(ABORT, 'snapshots are append-only');
END;

DROP TRIGGER review_decision_no_delete;
CREATE TRIGGER review_decision_no_delete
BEFORE DELETE ON review_decision
WHEN NOT EXISTS (
    SELECT 1
    FROM disposition_event de
    JOIN disposition_run dr ON dr.id = de.disposition_run_id
    WHERE de.evaluation_record_id = OLD.evaluation_record_id
      AND dr.state = 'db_committed_vacuum_pending'
)
BEGIN
    SELECT RAISE(ABORT, 'review decisions are append-only');
END;

DROP TRIGGER contributor_event_no_delete;
CREATE TRIGGER contributor_event_no_delete
BEFORE DELETE ON contributor_event
WHEN NOT EXISTS (
    SELECT 1
    FROM disposition_event de
    JOIN disposition_run dr ON dr.id = de.disposition_run_id
    WHERE de.evaluation_record_id = OLD.evaluation_record_id
      AND dr.state = 'db_committed_vacuum_pending'
)
BEGIN
    SELECT RAISE(ABORT, 'contributor events are append-only');
END;

DROP TRIGGER draft_rating_frozen_delete;
CREATE TRIGGER draft_rating_frozen_delete
BEFORE DELETE ON draft_rating
WHEN (SELECT frozen FROM evaluation_record_frozen
      WHERE evaluation_record_id = OLD.evaluation_record_id) = 1
    AND NOT EXISTS (
        SELECT 1
        FROM disposition_event de
        JOIN disposition_run dr ON dr.id = de.disposition_run_id
        WHERE de.evaluation_record_id = OLD.evaluation_record_id
          AND dr.state = 'db_committed_vacuum_pending'
    )
BEGIN
    SELECT RAISE(ABORT, 'a submitted or approved draft is frozen');
END;

DROP TRIGGER draft_rating_modifier_frozen_delete;
CREATE TRIGGER draft_rating_modifier_frozen_delete
BEFORE DELETE ON draft_rating_modifier
WHEN (SELECT frozen FROM evaluation_record_frozen
      WHERE evaluation_record_id = (SELECT evaluation_record_id
                                    FROM draft_rating
                                    WHERE id = OLD.draft_rating_id)) = 1
    AND NOT EXISTS (
        SELECT 1
        FROM disposition_event de
        JOIN disposition_run dr ON dr.id = de.disposition_run_id
        WHERE de.evaluation_record_id = (SELECT evaluation_record_id
                                         FROM draft_rating
                                         WHERE id = OLD.draft_rating_id)
          AND dr.state = 'db_committed_vacuum_pending'
    )
BEGIN
    SELECT RAISE(ABORT, 'a submitted or approved draft is frozen');
END;

DROP TRIGGER draft_narrative_frozen_delete;
CREATE TRIGGER draft_narrative_frozen_delete
BEFORE DELETE ON draft_narrative
WHEN (SELECT frozen FROM evaluation_record_frozen
      WHERE evaluation_record_id = OLD.evaluation_record_id) = 1
    AND NOT EXISTS (
        SELECT 1
        FROM disposition_event de
        JOIN disposition_run dr ON dr.id = de.disposition_run_id
        WHERE de.evaluation_record_id = OLD.evaluation_record_id
          AND dr.state = 'db_committed_vacuum_pending'
    )
BEGIN
    SELECT RAISE(ABORT, 'a submitted or approved draft is frozen');
END;

DROP TRIGGER evaluation_session_frozen_delete;
CREATE TRIGGER evaluation_session_frozen_delete
BEFORE DELETE ON evaluation_session
WHEN (SELECT frozen FROM evaluation_record_frozen
      WHERE evaluation_record_id = OLD.evaluation_record_id) = 1
    AND NOT EXISTS (
        SELECT 1
        FROM disposition_event de
        JOIN disposition_run dr ON dr.id = de.disposition_run_id
        WHERE de.evaluation_record_id = OLD.evaluation_record_id
          AND dr.state = 'db_committed_vacuum_pending'
    )
BEGIN
    SELECT RAISE(ABORT, 'a submitted or approved draft is frozen');
END;

DROP TRIGGER summary_daily_link_frozen_delete;
CREATE TRIGGER summary_daily_link_frozen_delete
BEFORE DELETE ON summary_daily_link
WHEN (SELECT frozen FROM evaluation_record_frozen
      WHERE evaluation_record_id = OLD.summary_record_id) = 1
    AND NOT EXISTS (
        SELECT 1
        FROM disposition_event de
        JOIN disposition_run dr ON dr.id = de.disposition_run_id
        WHERE de.evaluation_record_id = OLD.summary_record_id
          AND dr.state = 'db_committed_vacuum_pending'
    )
BEGIN
    SELECT RAISE(ABORT, 'a submitted or approved draft is frozen');
END;

CREATE TRIGGER evaluation_record_no_delete
BEFORE DELETE ON evaluation_record
WHEN NOT EXISTS (
    SELECT 1
    FROM disposition_event de
    JOIN disposition_run dr ON dr.id = de.disposition_run_id
    WHERE de.evaluation_record_id = OLD.id
      AND dr.state = 'db_committed_vacuum_pending'
)
BEGIN
    SELECT RAISE(ABORT, 'evaluation records can only be deleted by an authorized disposition run');
END;
