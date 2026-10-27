-- Add semantic creation/update timestamps for local-source home listings.
-- created_at / updated_at are seconds since epoch and are independent of the
-- sync protocol's last_modified_at (which is a millisecond version counter).

ALTER TABLE manga ADD COLUMN created_at INTEGER;
ALTER TABLE manga ADD COLUMN updated_at INTEGER;
ALTER TABLE chapter ADD COLUMN created_at INTEGER;

-- Backfill existing rows from the closest available timestamps so upgraded
-- databases keep showing content instead of an empty home. For manga, sync's
-- last_modified_at (milliseconds) is used as an approximation of when the row
-- first appeared. updated_at stays NULL: only actual content changes after this
-- migration will populate it, so the latest-updates listing does not flood with
-- every pre-existing manga. For chapters, fetched_at is the only historical
-- timestamp; new scans will overwrite it with precise creation times.
UPDATE manga SET created_at = COALESCE(NULLIF(last_modified_at / 1000, 0), 0);
UPDATE chapter SET created_at = COALESCE(NULLIF(fetched_at, 0), 0);

CREATE INDEX manga_source_created_at ON manga (source_id, created_at);
CREATE INDEX manga_source_updated_at ON manga (source_id, updated_at);
CREATE INDEX chapter_manga_created_at ON chapter (manga_id, created_at);
