-- Whether a session's cookie was issued with the Secure attribute
-- (ADR 0025). Every session from before this migration was not. When the
-- service starts with an https:// public URL, it revokes live sessions
-- that were not, so a cookie a browser may still send over plain HTTP
-- stops being a credential at once rather than at its expiry.
ALTER TABLE session
    ADD COLUMN issued_secure INTEGER NOT NULL DEFAULT 0 CHECK (issued_secure IN (0, 1));
