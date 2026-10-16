ALTER TABLE sub2_managed_pools
  ADD COLUMN connection_type TEXT NOT NULL DEFAULT 'mixed';

UPDATE sub2_managed_pools
SET connection_type = CASE
  WHEN platform = 'openai' AND EXISTS (
    SELECT 1
    FROM user_providers
    WHERE user_providers.id = sub2_managed_pools.provider_id
      AND user_providers.name LIKE '%Codex%'
  ) THEN 'subscription'
  WHEN platform = 'openai' THEN 'api_key'
  ELSE 'mixed'
END;

CREATE INDEX IF NOT EXISTS idx_sub2_managed_pools_connection
  ON sub2_managed_pools(user_id, platform, connection_type, provider_id);
