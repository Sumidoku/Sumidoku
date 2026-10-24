CREATE TABLE app_event (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    kind TEXT NOT NULL,
    payload TEXT NOT NULL,
    user_id INTEGER,
    created_at INTEGER NOT NULL
);

CREATE INDEX idx_app_event_user_id_id ON app_event (user_id, id);
CREATE INDEX idx_app_event_created_at ON app_event (created_at);
