-- 单本隐身：开启后该漫画的进度写入不再刷新 chapter_user.last_read_at，历史保持现状
ALTER TABLE manga_user ADD COLUMN incognito INTEGER NOT NULL DEFAULT 0;
