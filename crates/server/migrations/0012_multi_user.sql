-- Multi-user identity groundwork: the user table becomes the sole credential carrier (auth_credential is retired), and tokens belong to users.
-- SQLite with foreign keys enabled disallows ADD COLUMN with REFERENCES and a non-NULL default, so both tables are rebuilt
CREATE TABLE auth_token_new (
    token_hash TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES user (id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
);
INSERT INTO auth_token_new (token_hash, user_id, created_at, expires_at)
SELECT token_hash, 1, created_at, expires_at FROM auth_token;
DROP TABLE auth_token;
ALTER TABLE auth_token_new RENAME TO auth_token;

-- Moves the site password into the admin user: the same password can log in directly after migration
UPDATE user SET password_hash = (SELECT password_hash FROM auth_credential WHERE id = 1)
WHERE id = 1 AND EXISTS (SELECT 1 FROM auth_credential WHERE id = 1);

DROP TABLE auth_credential;

-- download becomes per-user rows: artifact files are shared, rows belong to their respective users
CREATE TABLE download_new (
    chapter_id INTEGER NOT NULL REFERENCES chapter (id) ON DELETE CASCADE,
    user_id INTEGER NOT NULL REFERENCES user (id) ON DELETE CASCADE,
    status TEXT NOT NULL DEFAULT 'queued',
    format TEXT NOT NULL DEFAULT 'cbz',
    path TEXT,
    error TEXT,
    retries INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (chapter_id, user_id)
);
INSERT INTO download_new (chapter_id, user_id, status, format, path, error, retries, created_at, updated_at)
SELECT chapter_id, user_id, status, format, path, error, retries, created_at, updated_at FROM download;
DROP TABLE download;
ALTER TABLE download_new RENAME TO download;

-- The incognito switch moves into the user.<id> namespace; the read side switches over when user scoping lands
UPDATE setting SET key = 'user.1.incognito.enabled' WHERE key = 'incognito.enabled';
