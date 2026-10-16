CREATE TABLE registration_codes (
    email TEXT PRIMARY KEY,
    generation TEXT NOT NULL,
    code_hash TEXT NOT NULL,
    expires_at INTEGER NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    sent INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE registration_email_sends (
    generation TEXT PRIMARY KEY,
    email TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX registration_sends_email_time ON registration_email_sends(email, created_at);
CREATE INDEX registration_sends_time ON registration_email_sends(created_at);
