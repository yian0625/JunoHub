-- Display names are not identities. Different clients may use the same preset
-- name (for example JunoHub); each configuration retains its independent ID.
CREATE TABLE integration_profiles_v2 (
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
  base_url TEXT NOT NULL DEFAULT '',
  encrypted_api_key TEXT,
  sort_order INTEGER NOT NULL DEFAULT 0,
  metadata TEXT NOT NULL DEFAULT '{}',
  encrypted_settings TEXT
);
INSERT INTO integration_profiles_v2
  (id,user_id,name,provider_type,virtual_key_id,apps,models,website_url,notes,
   created_at,updated_at,base_url,encrypted_api_key,sort_order,metadata,encrypted_settings)
SELECT id,user_id,name,provider_type,virtual_key_id,apps,models,website_url,notes,
       created_at,updated_at,base_url,encrypted_api_key,sort_order,metadata,encrypted_settings
FROM integration_profiles;
DROP TABLE integration_profiles;
ALTER TABLE integration_profiles_v2 RENAME TO integration_profiles;
CREATE INDEX idx_integration_profiles_owner
  ON integration_profiles(user_id, updated_at DESC);
