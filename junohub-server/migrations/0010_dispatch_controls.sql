ALTER TABLE provider_dispatch_policy RENAME TO provider_dispatch_policy_old;
CREATE TABLE provider_dispatch_policy (
 provider_id INTEGER PRIMARY KEY REFERENCES user_providers(id) ON DELETE CASCADE,
 strategy TEXT NOT NULL CHECK(strategy IN ('balanced','priority','round_robin','weighted')),
 max_attempts INTEGER NOT NULL CHECK(max_attempts BETWEEN 1 AND 100),
 sticky_sessions INTEGER NOT NULL CHECK(sticky_sessions IN (0,1)),
 request_timeout_seconds INTEGER NOT NULL DEFAULT 120 CHECK(request_timeout_seconds BETWEEN 5 AND 600),
 failure_cooldown_seconds INTEGER NOT NULL DEFAULT 30 CHECK(failure_cooldown_seconds BETWEEN 5 AND 3600)
);
INSERT INTO provider_dispatch_policy(provider_id,strategy,max_attempts,sticky_sessions)
 SELECT provider_id,strategy,max_attempts,sticky_sessions FROM provider_dispatch_policy_old;
DROP TABLE provider_dispatch_policy_old;
