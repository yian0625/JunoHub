-- Keep credentials after removing them from dispatch groups.
ALTER TABLE provider_keys ADD COLUMN assigned INTEGER NOT NULL DEFAULT 1 CHECK(assigned IN (0,1));
CREATE TRIGGER provider_keys_unique_display_name_insert BEFORE INSERT ON provider_keys
WHEN EXISTS(SELECT 1 FROM provider_keys WHERE provider_id=NEW.provider_id AND lower(trim(name))=lower(trim(NEW.name)))
BEGIN SELECT RAISE(ABORT,'duplicate account name'); END;
CREATE TRIGGER provider_keys_unique_display_name_update BEFORE UPDATE OF name ON provider_keys
WHEN EXISTS(SELECT 1 FROM provider_keys WHERE provider_id=NEW.provider_id AND id<>NEW.id AND lower(trim(name))=lower(trim(NEW.name)))
BEGIN SELECT RAISE(ABORT,'duplicate account name'); END;
