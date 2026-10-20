-- Per-manga incognito: once enabled, progress writes for that manga no longer refresh chapter_user.last_read_at; history stays as-is
ALTER TABLE manga_user ADD COLUMN incognito INTEGER NOT NULL DEFAULT 0;
