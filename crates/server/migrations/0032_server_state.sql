-- Instance state only the server writes moves out of setting, so the settings API and backups
-- no longer need to know about it. The origin trigger reads the instance id at runtime, so it moves too.
CREATE TABLE server_state (
    key TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
);

INSERT INTO server_state (key, value)
SELECT key, value FROM setting
WHERE key IN (
    'sync.instance_id',
    'sync.single_owner_rebaseline',
    'backup.last_run',
    'library.last_scheduled_run_at',
    'library.last_update_finished_at',
    'local.last_scan_at'
) AND value != 'null';

DELETE FROM setting
WHERE key IN (
    'sync.instance_id',
    'sync.single_owner_rebaseline',
    'backup.last_run',
    'library.last_scheduled_run_at',
    'library.last_update_finished_at',
    'local.last_scan_at'
);

DROP TRIGGER sync_change_origin_insert;

CREATE TRIGGER sync_change_origin_insert AFTER INSERT ON sync_change
WHEN NEW.origin_sequence = 0
BEGIN
    UPDATE sync_change
    SET origin = (SELECT value FROM server_state WHERE key = 'sync.instance_id'),
        origin_user_id = NEW.user_id,
        origin_entity_key = NEW.entity_key,
        origin_sequence = NEW.sequence
    WHERE sequence = NEW.sequence;
    INSERT INTO sync_alias (origin, origin_user_id, user_id, entity_kind, origin_entity_key, entity_key)
    SELECT origin, origin_user_id, user_id, entity_kind, origin_entity_key, entity_key
    FROM sync_change WHERE sequence = NEW.sequence
    ON CONFLICT (origin, origin_user_id, user_id, entity_kind, origin_entity_key)
    DO UPDATE SET entity_key = excluded.entity_key;
END;
