CREATE TABLE integration_profiles (
  id TEXT PRIMARY KEY,
  user_id INTEGER NOT NULL,
  name TEXT NOT NULL,
  provider_type TEXT NOT NULL DEFAULT 'junohub',
  virtual_key_id INTEGER NOT NULL,
  apps TEXT NOT NULL DEFAULT '{"claude":true,"codex":true,"gemini":true}',
  models TEXT NOT NULL DEFAULT '{}',
  website_url TEXT NOT NULL DEFAULT '',
  notes TEXT NOT NULL DEFAULT '',
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  UNIQUE(user_id, name)
);

CREATE INDEX idx_integration_profiles_owner
  ON integration_profiles(user_id, updated_at DESC);

ALTER TABLE client_connect_tokens
  ADD COLUMN config_json TEXT NOT NULL DEFAULT '{}';
