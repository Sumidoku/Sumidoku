-- Local-source lifecycle (kmrs trash bin analogue): a trashed row keeps its content, progress and page
-- cache until purged; a scan that sees it again clears the flag in place. scan_mtime is the scan's
-- change detector: max directory mtime of the manga subtree (plus chapter file mtimes when the
-- instance's forceModifiedTime is on), so an unchanged manga skips reconciliation entirely.
-- All three are local-source bookkeeping: the sync triggers list their tracked columns explicitly
-- (migration 0017), so writes to these columns produce no sync events.
ALTER TABLE manga ADD COLUMN trashed_at INTEGER;
ALTER TABLE manga ADD COLUMN scan_mtime INTEGER;
ALTER TABLE chapter ADD COLUMN trashed_at INTEGER;
