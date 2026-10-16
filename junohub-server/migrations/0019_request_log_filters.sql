ALTER TABLE user_request_logs ADD COLUMN provider_id INTEGER NOT NULL DEFAULT 0;
ALTER TABLE user_request_logs ADD COLUMN provider_name TEXT NOT NULL DEFAULT '';

CREATE INDEX IF NOT EXISTS idx_user_request_logs_filters
  ON user_request_logs(user_id, model_alias, provider_name, status_code, created_at DESC);
