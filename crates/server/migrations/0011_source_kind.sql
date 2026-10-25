-- M11 splits source into kinds: aidoku (wasm online source, default for existing rows) and local (built-in local source).
-- The local source reuses the shared manga/chapter/page tables and has no wasm payload, so wasm_path holds an empty-string placeholder
ALTER TABLE source ADD COLUMN kind TEXT NOT NULL DEFAULT 'aidoku';

INSERT OR IGNORE INTO source (key, name, kind, version, languages, wasm_path, enabled, installed_at, updated_at)
VALUES ('local.local', 'Local', 'local', 0, '[]', '', 1, 0, 0);
