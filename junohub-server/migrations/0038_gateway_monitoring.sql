ALTER TABLE user_request_logs ADD COLUMN request_id TEXT NOT NULL DEFAULT '';
CREATE INDEX user_request_logs_request_id ON user_request_logs(request_id) WHERE request_id != '';
CREATE INDEX user_request_logs_monitor ON user_request_logs(created_at,status_code,provider_id);

CREATE TABLE key_limits (
 id INTEGER PRIMARY KEY REFERENCES virtual_keys(id) ON DELETE CASCADE,
 max_concurrency INTEGER NOT NULL DEFAULT 0 CHECK(max_concurrency BETWEEN 0 AND 100000),
 rpm INTEGER NOT NULL DEFAULT 0 CHECK(rpm BETWEEN 0 AND 1000000)
);
CREATE TABLE monitor_settings (
 id INTEGER PRIMARY KEY CHECK(id=1),
 revision INTEGER NOT NULL DEFAULT 0,
 config_json TEXT NOT NULL DEFAULT '{}',
 updated_at INTEGER NOT NULL DEFAULT 0
);
INSERT INTO monitor_settings(id) VALUES(1);
CREATE TABLE monitor_snapshots (
 id INTEGER PRIMARY KEY CHECK(id=1),
 data_json TEXT NOT NULL,
 updated_at INTEGER NOT NULL
);
CREATE TABLE worker_heartbeats (
 instance_id TEXT NOT NULL,
 name TEXT NOT NULL,
 enabled INTEGER NOT NULL,
 last_tick INTEGER NOT NULL DEFAULT 0,
 last_success INTEGER NOT NULL DEFAULT 0,
 message TEXT NOT NULL DEFAULT '',
 PRIMARY KEY(instance_id,name)
);
CREATE TABLE monitor_alerts (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 fingerprint TEXT NOT NULL,
 rule TEXT NOT NULL,
 severity TEXT NOT NULL,
 title TEXT NOT NULL,
 detail TEXT NOT NULL,
 opened_at INTEGER NOT NULL,
 last_seen_at INTEGER NOT NULL,
 resolved_at INTEGER NOT NULL DEFAULT 0,
 acknowledged_at INTEGER NOT NULL DEFAULT 0,
 acknowledged_by INTEGER NOT NULL DEFAULT 0,
 occurrences INTEGER NOT NULL DEFAULT 1
);
CREATE UNIQUE INDEX monitor_alerts_active ON monitor_alerts(fingerprint) WHERE resolved_at=0;
CREATE INDEX monitor_alerts_history ON monitor_alerts(opened_at DESC);
CREATE TABLE operation_audit (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 actor_id INTEGER NOT NULL,
 action TEXT NOT NULL,
 resource_id INTEGER NOT NULL DEFAULT 0,
 request_id TEXT NOT NULL DEFAULT '',
 status_code INTEGER NOT NULL,
 created_at INTEGER NOT NULL
);
CREATE INDEX operation_audit_time ON operation_audit(created_at DESC);
