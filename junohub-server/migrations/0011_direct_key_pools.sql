CREATE TABLE provider_keys (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 provider_id INTEGER NOT NULL REFERENCES user_providers(id) ON DELETE CASCADE,
 name TEXT NOT NULL,
 encrypted_api_key TEXT NOT NULL DEFAULT '',
 base_url TEXT NOT NULL,
 priority INTEGER NOT NULL DEFAULT 1 CHECK(priority BETWEEN 1 AND 999),
 weight INTEGER NOT NULL DEFAULT 1 CHECK(weight BETWEEN 1 AND 1000),
 concurrency INTEGER NOT NULL DEFAULT 10 CHECK(concurrency BETWEEN 1 AND 1000),
 rpm INTEGER NOT NULL DEFAULT 0 CHECK(rpm BETWEEN 0 AND 1000000),
 allowed_models TEXT NOT NULL DEFAULT '[]',
 cooldown_until INTEGER NOT NULL DEFAULT 0,
 health TEXT NOT NULL DEFAULT 'ready',
 UNIQUE(provider_id,name)
);
-- Preserve each direct connection, including deliberately unauthenticated endpoints.
INSERT INTO provider_keys(provider_id,name,encrypted_api_key,base_url)
 SELECT p.id,'默认 Key',p.encrypted_api_key,p.base_url FROM user_providers p
 WHERE NOT EXISTS(SELECT 1 FROM sub2_managed_pools m WHERE m.provider_id=p.id)
 AND p.provider_type<>'sub2api';
CREATE TABLE provider_limits (
 id INTEGER PRIMARY KEY REFERENCES user_providers(id) ON DELETE CASCADE,
 max_concurrency INTEGER NOT NULL DEFAULT 0 CHECK(max_concurrency BETWEEN 0 AND 100000),
 rpm INTEGER NOT NULL DEFAULT 0 CHECK(rpm BETWEEN 0 AND 1000000)
);
CREATE TABLE model_limits (
 id INTEGER PRIMARY KEY REFERENCES user_models(id) ON DELETE CASCADE,
 max_concurrency INTEGER NOT NULL DEFAULT 0 CHECK(max_concurrency BETWEEN 0 AND 100000),
 rpm INTEGER NOT NULL DEFAULT 0 CHECK(rpm BETWEEN 0 AND 1000000)
);

UPDATE user_providers SET encrypted_api_key='' WHERE provider_type<>'sub2api' AND id IN (SELECT provider_id FROM provider_keys);
