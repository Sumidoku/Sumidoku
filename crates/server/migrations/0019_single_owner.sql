CREATE TEMP VIEW sync_single_owner_tombstone AS
SELECT sequence
FROM (
    SELECT sequence, origin_user_id, deleted,
        ROW_NUMBER() OVER (
            PARTITION BY user_id, entity_kind, entity_key ORDER BY sequence DESC
        ) AS rn
    FROM sync_change
    WHERE user_id = 1
)
WHERE rn = 1 AND origin_user_id != 1 AND deleted = 1;
UPDATE sync_change
SET origin = (SELECT value FROM setting WHERE key = 'sync.instance_id'),
    origin_user_id = 1,
    origin_entity_key = entity_key,
    origin_sequence = sequence,
    identity = CASE
        WHEN entity_kind IN ('manga_user', 'chapter_user', 'category', 'category_manga')
        THEN json_set(identity, '$.userId', 1) ELSE identity END,
    snapshot = CASE
        WHEN entity_kind IN ('manga_user', 'chapter_user', 'category', 'category_manga')
        THEN json_set(snapshot, '$.userId', 1) ELSE snapshot END
WHERE sequence IN (SELECT sequence FROM sync_single_owner_tombstone);
DROP VIEW sync_single_owner_tombstone;
DELETE FROM sync_change WHERE origin_user_id != 1;
DELETE FROM sync_alias WHERE origin_user_id != 1;
DELETE FROM sync_origin_state WHERE origin_user_id != 1;
DELETE FROM setting WHERE key LIKE 'user.%' AND key NOT LIKE 'user.1.%';
UPDATE user SET role = 'admin' WHERE id = 1;
DELETE FROM user WHERE id != 1;
INSERT OR IGNORE INTO setting (key, value) VALUES ('sync.single_owner_rebaseline', 'true');
