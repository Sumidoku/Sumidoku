-- 单用户凭证：密码只存 argon2 哈希，单行
CREATE TABLE auth_credential (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    password_hash TEXT NOT NULL
);

-- 会话 token 只存 sha256 指纹，明文仅在登录响应里出现一次
CREATE TABLE auth_token (
    token_hash TEXT PRIMARY KEY,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
);
