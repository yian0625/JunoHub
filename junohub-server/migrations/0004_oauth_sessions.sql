CREATE TABLE IF NOT EXISTS oauth_sessions (
  session_hash TEXT PRIMARY KEY,
  owner_id INTEGER NOT NULL,
  provider TEXT NOT NULL,
  state_hash TEXT NOT NULL,
  encrypted_payload TEXT NOT NULL,
  expires_at INTEGER NOT NULL,
  created_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_oauth_sessions_expiry
  ON oauth_sessions(expires_at);
