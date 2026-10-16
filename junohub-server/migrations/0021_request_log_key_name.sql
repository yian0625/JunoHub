ALTER TABLE user_request_logs ADD COLUMN key_name TEXT NOT NULL DEFAULT '';

UPDATE user_request_logs
SET key_name = COALESCE((
  SELECT k.name
  FROM virtual_keys k
  WHERE k.id = user_request_logs.virtual_key_id
    AND k.user_id = user_request_logs.user_id
  LIMIT 1
), '')
WHERE key_name = '';
