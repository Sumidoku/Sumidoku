-- Persistent background task queue (kmrs TASK analogue). The string id doubles as the
-- deduplication key (e.g. 'local_scan:3'), so enqueueing an already-queued task bumps its
-- priority instead of piling up. Pending rows survive a restart and are re-dispatched at
-- boot; finished rows keep a bounded history for the tasks API.
CREATE TABLE task (
	id TEXT PRIMARY KEY,
	kind TEXT NOT NULL,
	-- Group key for serialization: tasks of one local source never run concurrently; NULL is the global group
	source_id INTEGER REFERENCES source (id) ON DELETE CASCADE,
	payload TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(payload)),
	priority INTEGER NOT NULL DEFAULT 4,
	status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'running', 'done', 'failed', 'cancelled')),
	created_at INTEGER NOT NULL,
	started_at INTEGER,
	finished_at INTEGER,
	error TEXT,
	result TEXT CHECK (result IS NULL OR json_valid(result))
);
CREATE INDEX task_pending_pick ON task (status, priority DESC, created_at);
