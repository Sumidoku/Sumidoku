-- 多用户身份地基：user 表成为唯一凭证载体（auth_credential 退场），token 归属用户。
-- SQLite 在外键开启时不允许 ADD COLUMN 带 REFERENCES 且默认值非 NULL，两表都重建
CREATE TABLE auth_token_new (
    token_hash TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES user (id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
);
INSERT INTO auth_token_new (token_hash, user_id, created_at, expires_at)
SELECT token_hash, 1, created_at, expires_at FROM auth_token;
DROP TABLE auth_token;
ALTER TABLE auth_token_new RENAME TO auth_token;

-- 站点密码搬进 admin 用户：迁移后同密码可直接登录
UPDATE user SET password_hash = (SELECT password_hash FROM auth_credential WHERE id = 1)
WHERE id = 1 AND EXISTS (SELECT 1 FROM auth_credential WHERE id = 1);

DROP TABLE auth_credential;

-- download 改为按用户持行：产物文件共享，行归属各自用户
CREATE TABLE download_new (
    chapter_id INTEGER NOT NULL REFERENCES chapter (id) ON DELETE CASCADE,
    user_id INTEGER NOT NULL REFERENCES user (id) ON DELETE CASCADE,
    status TEXT NOT NULL DEFAULT 'queued',
    format TEXT NOT NULL DEFAULT 'cbz',
    path TEXT,
    error TEXT,
    retries INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (chapter_id, user_id)
);
INSERT INTO download_new (chapter_id, user_id, status, format, path, error, retries, created_at, updated_at)
SELECT chapter_id, user_id, status, format, path, error, retries, created_at, updated_at FROM download;
DROP TABLE download;
ALTER TABLE download_new RENAME TO download;

-- 隐身开关收进 user.<id> 命名空间；读取侧在用户作用域落地时切换
UPDATE setting SET key = 'user.1.incognito.enabled' WHERE key = 'incognito.enabled';
