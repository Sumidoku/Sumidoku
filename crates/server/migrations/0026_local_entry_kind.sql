-- The recorded filesystem shape distinguishes a directory from an archive when the same key later names
-- a different entry kind. NULL keeps existing rows unknown until a scan observes them.
ALTER TABLE manga ADD COLUMN local_entry_kind TEXT
	CHECK (local_entry_kind IN ('directory', 'archive'));
