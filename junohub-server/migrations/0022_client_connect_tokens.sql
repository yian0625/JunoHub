CREATE TABLE IF NOT EXISTS client_connect_tokens (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  token_hash TEXT NOT NULL UNIQUE,
  user_id INTEGER NOT NULL,
  virtual_key_id INTEGER NOT NULL,
  client TEXT NOT NULL,
  model_alias TEXT NOT NULL,
  expires_at INTEGER NOT NULL,
  consumed_at INTEGER,
  created_at INTEGER NOT NULL,
  FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE,
  FOREIGN KEY(virtual_key_id) REFERENCES virtual_keys(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_client_connect_tokens_expiry
  ON client_connect_tokens(expires_at, consumed_at);
