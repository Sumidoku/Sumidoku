-- 源侧阅读方向与章节缩略图持久化；viewer 取值同 aidoku Viewer 枚举（0=Unknown 起）
ALTER TABLE manga ADD COLUMN viewer INTEGER NOT NULL DEFAULT 0;
ALTER TABLE chapter ADD COLUMN thumbnail TEXT;
