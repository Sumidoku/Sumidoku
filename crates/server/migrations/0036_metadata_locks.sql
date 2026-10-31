-- Field locks follow an entry through source refreshes, backups and sync, so
-- they live on the row the rest of its metadata lives on.
ALTER TABLE manga ADD COLUMN metadata_locks TEXT NOT NULL DEFAULT '{}';
