CREATE TABLE oauth_completions (
    session_hash TEXT PRIMARY KEY,
    binding_hash TEXT NOT NULL,
    phase TEXT NOT NULL,
    encrypted_token TEXT,
    encrypted_result TEXT,
    expires_at INTEGER NOT NULL
);
