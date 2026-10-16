ALTER TABLE virtual_keys ADD COLUMN external_ref TEXT NOT NULL DEFAULT '';
CREATE UNIQUE INDEX IF NOT EXISTS idx_virtual_keys_external_ref
  ON virtual_keys(user_id, external_ref) WHERE external_ref <> '';
