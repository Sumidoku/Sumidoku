-- Aidoku-style filter groups: a named, persisted rule set compiled into the library
-- query's WHERE clause. One rule row per (type, value) keeps same-dimension includes
-- naturally OR-able; NULL value serves the value-less flag rules (downloaded,
-- has_unread, started, completed).
CREATE TABLE filter_group (
    id INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES user (id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    sort_order INTEGER NOT NULL DEFAULT 0,
    UNIQUE (user_id, name)
);

CREATE TABLE filter_group_rule (
    id INTEGER PRIMARY KEY,
    group_id INTEGER NOT NULL REFERENCES filter_group (id) ON DELETE CASCADE,
    type TEXT NOT NULL,
    value TEXT,
    exclude INTEGER NOT NULL DEFAULT 0,
    UNIQUE (group_id, type, value)
);
