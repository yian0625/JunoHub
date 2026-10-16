ALTER TABLE integration_profiles ADD COLUMN sort_order INTEGER NOT NULL DEFAULT 0;

UPDATE integration_profiles
SET sort_order = created_at
WHERE sort_order = 0;
