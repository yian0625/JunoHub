DROP INDEX IF EXISTS idx_sub2_managed_pools_connection;

ALTER TABLE sub2_managed_pools
  DROP COLUMN connection_type;
