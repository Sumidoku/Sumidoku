-- Track cache recency and revisions for each chapter's derived page assets.
CREATE TABLE chapter_cache_state (
    chapter_id INTEGER PRIMARY KEY REFERENCES chapter (id) ON DELETE CASCADE,
    bytes INTEGER NOT NULL DEFAULT 0,
    accessed_at INTEGER NOT NULL DEFAULT 0,
    revision INTEGER NOT NULL DEFAULT 1,
    last_error TEXT
);

-- Hiding a terminal download from the queue preserves its offline product.
ALTER TABLE download ADD COLUMN dismissed_at INTEGER;

-- Global cache epoch returned by cache management endpoints.
CREATE TABLE chapter_cache_meta (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    revision INTEGER NOT NULL DEFAULT 1
);
INSERT INTO chapter_cache_meta (id, revision) VALUES (1, 1);
