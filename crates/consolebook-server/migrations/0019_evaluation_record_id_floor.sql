-- ADR 0030; #213: monotonic floor preventing new evaluation_record rows from
-- ever reusing an evaluation_record.id that was lawfully disposed (either in
-- this database's disposition_event table or recorded in a recovery ledger
-- during restore onto a snapshot that predates those records).

CREATE TABLE evaluation_record_id_floor (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    last_reserved_id INTEGER NOT NULL CHECK (last_reserved_id >= 0)
) STRICT;

INSERT INTO evaluation_record_id_floor (id, last_reserved_id)
VALUES (1, COALESCE((SELECT MAX(evaluation_record_id) FROM disposition_event), 0));

CREATE TRIGGER evaluation_record_id_floor_no_delete
BEFORE DELETE ON evaluation_record_id_floor
BEGIN
    SELECT RAISE(ABORT, 'evaluation record id floor cannot be deleted');
END;

CREATE TRIGGER evaluation_record_id_floor_monotonic
BEFORE UPDATE ON evaluation_record_id_floor
WHEN NEW.id != OLD.id OR NEW.last_reserved_id < OLD.last_reserved_id
BEGIN
    SELECT RAISE(ABORT, 'evaluation record id floor advances monotonically');
END;

CREATE TRIGGER disposition_event_advances_id_floor
AFTER INSERT ON disposition_event
BEGIN
    UPDATE evaluation_record_id_floor
    SET last_reserved_id = MAX(last_reserved_id, NEW.evaluation_record_id)
    WHERE id = 1;
END;

-- AFTER INSERT runs with the actual assigned INTEGER PRIMARY KEY (whether
-- provided explicitly or auto-assigned by SQLite), and RAISE(ABORT) rolls back
-- the statement atomically.
CREATE TRIGGER evaluation_record_no_disposed_id_reuse
AFTER INSERT ON evaluation_record
WHEN NEW.id <= (SELECT last_reserved_id FROM evaluation_record_id_floor WHERE id = 1)
   OR EXISTS (SELECT 1 FROM disposition_event WHERE evaluation_record_id = NEW.id)
BEGIN
    SELECT RAISE(ABORT, 'evaluation record id cannot reuse a disposed or reserved record id');
END;
