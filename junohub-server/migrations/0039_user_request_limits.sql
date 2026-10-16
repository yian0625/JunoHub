-- Administrator-managed limits are shared across every key owned by a user.
CREATE TABLE IF NOT EXISTS user_limits (
    id INTEGER PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    max_concurrency INTEGER NOT NULL DEFAULT 0 CHECK (max_concurrency BETWEEN 0 AND 100000),
    rpm INTEGER NOT NULL DEFAULT 0 CHECK (rpm BETWEEN 0 AND 1000000)
);
