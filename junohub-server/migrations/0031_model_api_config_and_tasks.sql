ALTER TABLE user_models ADD COLUMN api_config TEXT NOT NULL DEFAULT '{}';

CREATE TABLE gateway_tasks (
    id TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL,
    virtual_key_id INTEGER NOT NULL,
    model_id INTEGER NOT NULL,
    model_alias TEXT NOT NULL,
    provider_id INTEGER NOT NULL,
    provider_key_id INTEGER NOT NULL,
    base_url TEXT NOT NULL,
    encrypted_api_key TEXT NOT NULL,
    auth_type TEXT NOT NULL,
    task_path TEXT NOT NULL,
    upstream_task_id TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'submitting',
    result_json TEXT,
    log_id INTEGER,
    settled INTEGER NOT NULL DEFAULT 0,
    input_price REAL,
    output_price REAL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
);
CREATE INDEX gateway_tasks_expiry ON gateway_tasks(expires_at);
