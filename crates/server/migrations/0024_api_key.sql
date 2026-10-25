-- Web UI-managed API keys: store only the sha256 fingerprint; the plaintext appears only once, in the creation response; revocation deletes the row
CREATE TABLE api_key (
    key_hash TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    last_used_at INTEGER
);
