-- ADR 0030; #211: separate explicit disposition authority (`execute_disposition`)
-- tracked alongside retention administration (`manage_retention`) in the
-- append-only authority event log. Existing rows record `manage_retention`.
ALTER TABLE retention_authority_event
    ADD COLUMN capability TEXT NOT NULL DEFAULT 'manage_retention'
    CHECK (capability IN ('manage_retention', 'execute_disposition'));
