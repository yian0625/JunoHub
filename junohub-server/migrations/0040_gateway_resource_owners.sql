-- Supplier response IDs can expose private conversation state. Their ownership
-- must use the client key, even when many clients share one upstream engine key.
CREATE TABLE gateway_resource_owners (
    provider_id INTEGER NOT NULL REFERENCES user_providers(id) ON DELETE CASCADE,
    model_id INTEGER NOT NULL REFERENCES user_models(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('response', 'item')),
    resource_hash TEXT NOT NULL,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    key_id INTEGER NOT NULL REFERENCES virtual_keys(id) ON DELETE CASCADE,
    expires_at INTEGER NOT NULL,
    PRIMARY KEY (provider_id, kind, resource_hash)
);
CREATE INDEX gateway_resource_expiry ON gateway_resource_owners(expires_at);
