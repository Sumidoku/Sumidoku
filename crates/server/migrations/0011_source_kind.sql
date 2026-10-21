-- M11 source 分 kind：aidoku（wasm 在线源，存量默认）与 local（内置本地源）。
-- 本地源复用 manga/chapter/page 统一表，没有 wasm 载荷，wasm_path 置空串占位
ALTER TABLE source ADD COLUMN kind TEXT NOT NULL DEFAULT 'aidoku';

INSERT OR IGNORE INTO source (key, name, kind, version, languages, wasm_path, enabled, installed_at, updated_at)
VALUES ('local.local', 'Local', 'local', 0, '[]', '', 1, 0, 0);
