ALTER TABLE virtual_keys ADD COLUMN encrypted_key TEXT;
ALTER TABLE virtual_keys ADD COLUMN cost_usd REAL NOT NULL DEFAULT 0;
ALTER TABLE virtual_keys ADD COLUMN unpriced_requests INTEGER NOT NULL DEFAULT 0;
UPDATE virtual_keys SET unpriced_requests = total_requests;
ALTER TABLE user_request_logs ADD COLUMN cost_usd REAL;
