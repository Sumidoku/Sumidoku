-- Web UI 管理的 API key：只存 sha256 指纹，明文仅在创建响应里出现一次；吊销即删除行
CREATE TABLE api_key (
    key_hash TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    last_used_at INTEGER
);
