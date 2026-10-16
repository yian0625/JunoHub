CREATE TABLE IF NOT EXISTS sub2_managed_pools (
  provider_id INTEGER PRIMARY KEY,
  user_id INTEGER NOT NULL,
  platform TEXT NOT NULL,
  group_id INTEGER NOT NULL,
  api_key_id INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_sub2_managed_pools_owner_platform
  ON sub2_managed_pools(user_id, platform, provider_id);
