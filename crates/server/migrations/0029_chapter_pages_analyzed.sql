-- Remember that page analysis of a chapter ran and found no pages (a non-divina
-- EPUB, a broken archive or an empty directory), so the analysis pass stops
-- reopening it on every run. A scan that sees the manga change clears it.

ALTER TABLE chapter ADD COLUMN pages_analyzed_at INTEGER;
