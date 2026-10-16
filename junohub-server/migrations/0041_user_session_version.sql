-- Password and administrative identity changes revoke older login sessions.
ALTER TABLE users ADD COLUMN session_version INTEGER NOT NULL DEFAULT 0;
