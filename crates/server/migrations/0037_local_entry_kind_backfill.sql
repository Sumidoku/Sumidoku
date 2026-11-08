-- Backfill manga.local_entry_kind for local rows recorded before migration 0026 introduced the
-- column. A migration has no filesystem access, so the key suffix is the only signal: it is right
-- for every ordinary entry, and the pathological case (a directory named like an archive, e.g.
-- "Series.cbz") is corrected by the next scan that observes the entry. Only local-source rows are
-- touched; online rows keep NULL. After this backfill the recorded kind is the single shape
-- authority for restore and reconcile decisions.
UPDATE manga SET local_entry_kind = 'archive'
 WHERE local_entry_kind IS NULL
   AND source_id IN (SELECT id FROM source WHERE kind = 'local')
   AND (lower(key) LIKE '%.cbz' OR lower(key) LIKE '%.zip' OR lower(key) LIKE '%.epub');

UPDATE manga SET local_entry_kind = 'directory'
 WHERE local_entry_kind IS NULL
   AND source_id IN (SELECT id FROM source WHERE kind = 'local');
