-- Per-component chapter cache accounting: feeds the settings breakdown and per-source
-- reporting. Startup reconciliation recomputes every column for existing rows.
ALTER TABLE chapter_cache_state ADD COLUMN page_bytes INTEGER NOT NULL DEFAULT 0;
ALTER TABLE chapter_cache_state ADD COLUMN original_bytes INTEGER NOT NULL DEFAULT 0;
ALTER TABLE chapter_cache_state ADD COLUMN blob_bytes INTEGER NOT NULL DEFAULT 0;
ALTER TABLE chapter_cache_state ADD COLUMN opds_bytes INTEGER NOT NULL DEFAULT 0;
