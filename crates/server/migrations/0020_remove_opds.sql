CREATE TEMP TABLE removed_opds_source (
    key TEXT PRIMARY KEY
);
INSERT INTO removed_opds_source SELECT key FROM source WHERE kind = 'opds';

UPDATE setting
SET value = COALESCE((
    SELECT json_group_array(pinned.value)
    FROM json_each(setting.value) AS pinned
    WHERE NOT EXISTS (
        SELECT 1 FROM source WHERE kind = 'opds' AND key = pinned.value
    )
), '[]')
WHERE key = 'browse.pinned_sources'
  AND json_valid(value)
  AND json_type(value) = 'array';

-- A group with no surviving rules would match every title.
DELETE FROM filter_group
WHERE id IN (
    SELECT group_id FROM filter_group_rule
    WHERE type = 'source' AND value IN (SELECT CAST(id AS TEXT) FROM source WHERE kind = 'opds')
)
AND NOT EXISTS (
    SELECT 1 FROM filter_group_rule remaining
    WHERE remaining.group_id = filter_group.id
      AND NOT (
          remaining.type = 'source'
          AND remaining.value IN (SELECT CAST(id AS TEXT) FROM source WHERE kind = 'opds')
      )
);
DELETE FROM filter_group_rule
WHERE type = 'source' AND value IN (SELECT CAST(id AS TEXT) FROM source WHERE kind = 'opds');

DELETE FROM source WHERE kind = 'opds';

-- Cover cleanup runs after startup rebaseline and must include ids already reused by another source.
INSERT OR REPLACE INTO setting (key, value)
SELECT DISTINCT 'migration.remove_opds_cover.' || entity_key, ''
FROM sync_change
WHERE entity_kind = 'manga'
  AND json_extract(identity, '$.sourceKey') IN (SELECT key FROM removed_opds_source);

-- Retired keys are rejected before alias resolution, including aliases left by compaction or rebaseline.
INSERT OR REPLACE INTO setting (key, value)
SELECT 'sync.retired_sources', json_group_array(key) FROM removed_opds_source;

DELETE FROM sync_change
WHERE json_extract(identity, '$.sourceKey') IN (SELECT key FROM removed_opds_source);

DROP TABLE removed_opds_source;
