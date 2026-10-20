-- Single-user credential: stores only the argon2 password hash, single row
CREATE TABLE auth_credential (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    password_hash TEXT NOT NULL
);

-- Session tokens store only the sha256 fingerprint; the plaintext appears only once, in the login response
CREATE TABLE auth_token (
    token_hash TEXT PRIMARY KEY,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
);
