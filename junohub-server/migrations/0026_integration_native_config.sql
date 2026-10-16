ALTER TABLE integration_profiles ADD COLUMN metadata TEXT NOT NULL DEFAULT '{}';
ALTER TABLE integration_profiles ADD COLUMN encrypted_settings TEXT;
