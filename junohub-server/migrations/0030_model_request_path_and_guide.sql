ALTER TABLE user_models ADD COLUMN request_path TEXT NOT NULL DEFAULT '';

CREATE TABLE access_guide (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    content TEXT NOT NULL,
    revision INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL DEFAULT 0
);
INSERT INTO access_guide (id, content) VALUES (1, '');
