-- M2 源仓库：index 原文整存，repo 不可达时浏览/更新检测全靠这份缓存
CREATE TABLE source_repo (
    id INTEGER PRIMARY KEY,
    url TEXT NOT NULL UNIQUE,
    name TEXT,
    index_json TEXT,
    fetch_error TEXT,
    added_at INTEGER NOT NULL DEFAULT 0,
    fetched_at INTEGER
);
