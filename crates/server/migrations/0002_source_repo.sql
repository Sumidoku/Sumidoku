-- M2 source repos: the index is stored verbatim in full; when a repo is unreachable, browsing and update detection rely entirely on this cache
CREATE TABLE source_repo (
    id INTEGER PRIMARY KEY,
    url TEXT NOT NULL UNIQUE,
    name TEXT,
    index_json TEXT,
    fetch_error TEXT,
    added_at INTEGER NOT NULL DEFAULT 0,
    fetched_at INTEGER
);
