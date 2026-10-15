-- Sessions whose cookie was issued with the Secure attribute (ADR 0025);
-- a session without a row here was not, which includes every session from
-- before this migration. When the service starts with an https:// public
-- URL it revokes live sessions that have no row, so a cookie a browser may
-- still send over plain HTTP stops being a credential at once rather than
-- at its expiry. A separate owner, so the session table stays as it was.
CREATE TABLE session_secure_cookie (
    token_hash TEXT PRIMARY KEY REFERENCES session (token_hash) ON DELETE CASCADE
) STRICT;
