CREATE TABLE source_cookie (
    source_id INTEGER NOT NULL REFERENCES source (id) ON DELETE CASCADE,
    domain TEXT NOT NULL,
    name TEXT NOT NULL,
    value TEXT NOT NULL,
    PRIMARY KEY (source_id, domain, name)
);
