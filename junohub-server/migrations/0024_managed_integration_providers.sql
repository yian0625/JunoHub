ALTER TABLE integration_profiles ADD COLUMN base_url TEXT NOT NULL DEFAULT '';
ALTER TABLE integration_profiles ADD COLUMN encrypted_api_key TEXT;

CREATE TABLE client_connect_tokens_v2 (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  token_hash TEXT NOT NULL UNIQUE,
  user_id INTEGER NOT NULL,
  virtual_key_id INTEGER,
  client TEXT NOT NULL,
  model_alias TEXT NOT NULL,
  expires_at INTEGER NOT NULL,
  consumed_at INTEGER,
  created_at INTEGER NOT NULL,
  config_json TEXT NOT NULL DEFAULT '{}',
  encrypted_api_key TEXT,
  base_url TEXT NOT NULL DEFAULT '',
  FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE,
  FOREIGN KEY(virtual_key_id) REFERENCES virtual_keys(id) ON DELETE CASCADE
);

INSERT INTO client_connect_tokens_v2
  (id,token_hash,user_id,virtual_key_id,client,model_alias,expires_at,consumed_at,created_at,config_json)
SELECT id,token_hash,user_id,virtual_key_id,client,model_alias,expires_at,consumed_at,created_at,config_json
FROM client_connect_tokens;

DROP TABLE client_connect_tokens;
ALTER TABLE client_connect_tokens_v2 RENAME TO client_connect_tokens;

CREATE INDEX idx_client_connect_tokens_expiry
  ON client_connect_tokens(expires_at, consumed_at);
