CREATE TABLE provider_dispatch_policy (
    provider_id INTEGER PRIMARY KEY REFERENCES user_providers(id) ON DELETE CASCADE,
    strategy TEXT NOT NULL DEFAULT 'balanced' CHECK(strategy IN ('balanced','priority','round_robin')),
    max_attempts INTEGER NOT NULL DEFAULT 100 CHECK(max_attempts BETWEEN 1 AND 100),
    sticky_sessions INTEGER NOT NULL DEFAULT 1 CHECK(sticky_sessions IN (0,1))
);
