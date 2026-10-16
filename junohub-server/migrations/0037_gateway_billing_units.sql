ALTER TABLE user_models ADD COLUMN billing_config TEXT NOT NULL DEFAULT '{}';
ALTER TABLE gateway_tasks ADD COLUMN billing_config TEXT NOT NULL DEFAULT '{}';
ALTER TABLE user_request_logs ADD COLUMN billing_config TEXT NOT NULL DEFAULT '{}';
ALTER TABLE user_request_logs ADD COLUMN billing_details TEXT NOT NULL DEFAULT '{}';
