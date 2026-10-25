-- M1 initial schema; all timestamps are INTEGER unix seconds, all JSON columns are TEXT
CREATE TABLE user (
    id INTEGER PRIMARY KEY,
    username TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL DEFAULT '',
    role TEXT NOT NULL DEFAULT 'admin',
    session_version INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL DEFAULT 0
);
INSERT OR IGNORE INTO user (id, username, role, created_at) VALUES (1, 'admin', 'admin', 0);

CREATE TABLE source (
    id INTEGER PRIMARY KEY,
    key TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    version INTEGER NOT NULL DEFAULT 0,
    languages TEXT NOT NULL DEFAULT '[]',
    content_rating INTEGER NOT NULL DEFAULT 0,
    base_url TEXT,
    breaking_change_version INTEGER NOT NULL DEFAULT 0,
    icon_path TEXT,
    wasm_path TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    installed_at INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE source_setting (
    source_id INTEGER NOT NULL REFERENCES source (id) ON DELETE CASCADE,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    PRIMARY KEY (source_id, key)
);

CREATE TABLE manga (
    id INTEGER PRIMARY KEY,
    source_id INTEGER NOT NULL REFERENCES source (id) ON DELETE CASCADE,
    key TEXT NOT NULL,
    title TEXT NOT NULL DEFAULT '',
    authors TEXT NOT NULL DEFAULT '[]',
    artists TEXT NOT NULL DEFAULT '[]',
    description TEXT,
    tags TEXT NOT NULL DEFAULT '[]',
    status INTEGER NOT NULL DEFAULT 0,
    content_rating INTEGER NOT NULL DEFAULT 0,
    cover_url TEXT,
    url TEXT,
    initialized INTEGER NOT NULL DEFAULT 0,
    update_strategy INTEGER NOT NULL DEFAULT 0,
    next_update_time INTEGER,
    last_fetched_at INTEGER,
    chapters_last_fetched_at INTEGER,
    UNIQUE (source_id, key)
);

CREATE TABLE manga_user (
    manga_id INTEGER NOT NULL REFERENCES manga (id) ON DELETE CASCADE,
    user_id INTEGER NOT NULL REFERENCES user (id) ON DELETE CASCADE,
    in_library INTEGER NOT NULL DEFAULT 0,
    in_library_at INTEGER,
    viewer_flags INTEGER NOT NULL DEFAULT 0,
    sort_key TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (manga_id, user_id)
);

CREATE TABLE chapter (
    id INTEGER PRIMARY KEY,
    manga_id INTEGER NOT NULL REFERENCES manga (id) ON DELETE CASCADE,
    key TEXT NOT NULL,
    title TEXT,
    number REAL,
    volume REAL,
    scanlators TEXT NOT NULL DEFAULT '[]',
    language TEXT,
    url TEXT,
    date_uploaded INTEGER,
    source_order INTEGER NOT NULL DEFAULT 0,
    page_count INTEGER NOT NULL DEFAULT 0,
    locked INTEGER NOT NULL DEFAULT 0,
    fetched_at INTEGER NOT NULL DEFAULT 0,
    UNIQUE (manga_id, key)
);

CREATE TABLE chapter_user (
    chapter_id INTEGER NOT NULL REFERENCES chapter (id) ON DELETE CASCADE,
    user_id INTEGER NOT NULL REFERENCES user (id) ON DELETE CASCADE,
    last_page_read INTEGER NOT NULL DEFAULT 0,
    is_read INTEGER NOT NULL DEFAULT 0,
    is_bookmarked INTEGER NOT NULL DEFAULT 0,
    last_read_at INTEGER,
    is_downloaded INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (chapter_id, user_id)
);

CREATE TABLE page (
    chapter_id INTEGER NOT NULL REFERENCES chapter (id) ON DELETE CASCADE,
    idx INTEGER NOT NULL,
    content TEXT NOT NULL,
    PRIMARY KEY (chapter_id, idx)
);

CREATE TABLE category (
    id INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES user (id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    sort_order INTEGER NOT NULL DEFAULT 0,
    is_default INTEGER NOT NULL DEFAULT 0,
    include_in_update INTEGER NOT NULL DEFAULT 0,
    include_in_download INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE category_manga (
    category_id INTEGER NOT NULL REFERENCES category (id) ON DELETE CASCADE,
    manga_id INTEGER NOT NULL REFERENCES manga (id) ON DELETE CASCADE,
    user_id INTEGER NOT NULL REFERENCES user (id) ON DELETE CASCADE,
    PRIMARY KEY (category_id, manga_id, user_id)
);

CREATE TABLE download (
    chapter_id INTEGER PRIMARY KEY REFERENCES chapter (id) ON DELETE CASCADE,
    user_id INTEGER NOT NULL REFERENCES user (id) ON DELETE CASCADE,
    status TEXT NOT NULL DEFAULT 'queued',
    format TEXT NOT NULL DEFAULT 'cbz',
    path TEXT,
    error TEXT,
    retries INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE setting (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
