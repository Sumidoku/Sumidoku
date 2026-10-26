-- Tracker integration (#35): account tokens, OAuth handshakes, manga-to-entry bindings
-- and a recent-entries ledger. Tokens are plaintext rows by design; the data
-- directory's filesystem permissions are the protection boundary.
CREATE TABLE tracker_oauth_token (
    provider TEXT NOT NULL CHECK (provider IN ('anilist', 'mal', 'bangumi', 'mangabaka')),
    user_id INTEGER NOT NULL REFERENCES user (id) ON DELETE CASCADE,
    access_token TEXT NOT NULL,
    refresh_token TEXT,
    token_type TEXT,
    expires_at INTEGER,
    username TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (provider, user_id)
);

CREATE TABLE tracker_oauth_pending (
    provider TEXT NOT NULL CHECK (provider IN ('anilist', 'mal', 'bangumi', 'mangabaka')),
    nonce TEXT NOT NULL,
    user_id INTEGER NOT NULL REFERENCES user (id) ON DELETE CASCADE,
    verifier TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (provider, nonce)
);
CREATE INDEX tracker_oauth_pending_user ON tracker_oauth_pending (provider, user_id);

CREATE TABLE tracker_link (
    manga_id INTEGER NOT NULL REFERENCES manga (id) ON DELETE CASCADE,
    user_id INTEGER NOT NULL REFERENCES user (id) ON DELETE CASCADE,
    provider TEXT NOT NULL CHECK (provider IN ('anilist', 'mal', 'bangumi', 'mangabaka')),
    track_id TEXT NOT NULL,
    title TEXT,
    track_mode TEXT NOT NULL DEFAULT 'auto' CHECK (track_mode IN ('auto', 'chapter', 'volume')),
    chapter_offset INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (manga_id, user_id, provider)
);
CREATE INDEX tracker_link_user ON tracker_link (user_id);

CREATE TABLE tracker_ledger (
    provider TEXT NOT NULL CHECK (provider IN ('anilist', 'mal', 'bangumi', 'mangabaka')),
    user_id INTEGER NOT NULL REFERENCES user (id) ON DELETE CASCADE,
    track_id TEXT NOT NULL,
    title TEXT,
    url TEXT,
    cover_url TEXT,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (provider, user_id, track_id)
);
CREATE INDEX tracker_ledger_user ON tracker_ledger (user_id, updated_at);
