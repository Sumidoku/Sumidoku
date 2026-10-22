INSERT INTO setting (key, value)
SELECT
    'user.1.library.excluded_download_categories',
    json_group_array(id)
FROM category
WHERE user_id = 1 AND include_in_download = 2
GROUP BY user_id
ON CONFLICT (key) DO NOTHING;

INSERT INTO setting (key, value)
SELECT 'auto_download_new_chapters', 'false'
WHERE EXISTS (
    SELECT 1 FROM category WHERE user_id = 1 AND include_in_download = 1
)
ON CONFLICT (key) DO UPDATE SET value = excluded.value;

DROP TRIGGER category_sync_insert;
CREATE TRIGGER category_sync_insert AFTER INSERT ON category
BEGIN
    UPDATE category
    SET last_modified_at = CASE
        WHEN NEW.last_modified_at <> 0 THEN NEW.last_modified_at
        ELSE MAX(
            CAST(strftime('%s', 'now') AS INTEGER) * 1000 + CAST(substr(strftime('%f', 'now'), 4, 3) AS INTEGER),
            COALESCE((
                SELECT MAX(sc.modified_at) + 1 FROM sync_change sc
                WHERE sc.user_id = NEW.user_id AND sc.entity_kind = 'category'
                  AND sc.entity_key = CAST(NEW.id AS TEXT)
            ), 0)
        )
    END
    WHERE id = NEW.id;

    INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
    SELECT
        NEW.user_id,
        'category',
        CAST(NEW.id AS TEXT),
        0,
        (SELECT last_modified_at FROM category WHERE id = NEW.id),
        json_object('categoryId', NEW.id, 'userId', NEW.user_id),
        json_object(
            'id', NEW.id,
            'userId', NEW.user_id,
            'name', NEW.name,
            'sortOrder', NEW.sort_order,
            'isDefault', NEW.is_default,
            'includeInDownload', 0,
            'lastModifiedAt', (SELECT last_modified_at FROM category WHERE id = NEW.id)
        )
    WHERE EXISTS (SELECT 1 FROM user u WHERE u.id = NEW.user_id);
END;

DROP TRIGGER category_sync_update;
CREATE TRIGGER category_sync_update AFTER UPDATE ON category
WHEN
    OLD.user_id IS NOT NEW.user_id OR
    OLD.name IS NOT NEW.name OR
    OLD.sort_order IS NOT NEW.sort_order OR
    OLD.is_default IS NOT NEW.is_default
BEGIN
    UPDATE category
    SET last_modified_at = CASE
        WHEN NEW.last_modified_at <> 0 AND NEW.last_modified_at IS NOT OLD.last_modified_at THEN NEW.last_modified_at
        ELSE MAX(
            CAST(strftime('%s', 'now') AS INTEGER) * 1000 + CAST(substr(strftime('%f', 'now'), 4, 3) AS INTEGER),
            OLD.last_modified_at + 1,
            COALESCE((
                SELECT MAX(sc.modified_at) + 1 FROM sync_change sc
                WHERE sc.user_id = NEW.user_id AND sc.entity_kind = 'category'
                  AND sc.entity_key = CAST(NEW.id AS TEXT)
            ), 0)
        )
    END
    WHERE id = NEW.id;

    INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
    SELECT
        OLD.user_id,
        'category',
        CAST(OLD.id AS TEXT),
        1,
        (SELECT last_modified_at FROM category WHERE id = NEW.id),
        json_object('categoryId', OLD.id, 'userId', OLD.user_id),
        json_object(
            'id', OLD.id,
            'userId', OLD.user_id,
            'name', OLD.name,
            'sortOrder', OLD.sort_order,
            'isDefault', OLD.is_default,
            'includeInDownload', 0,
            'lastModifiedAt', OLD.last_modified_at
        )
    WHERE OLD.user_id IS NOT NEW.user_id
      AND EXISTS (SELECT 1 FROM user u WHERE u.id = OLD.user_id);

    INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
    SELECT
        NEW.user_id,
        'category',
        CAST(NEW.id AS TEXT),
        0,
        (SELECT last_modified_at FROM category WHERE id = NEW.id),
        json_object('categoryId', NEW.id, 'userId', NEW.user_id),
        json_object(
            'id', NEW.id,
            'userId', NEW.user_id,
            'name', NEW.name,
            'sortOrder', NEW.sort_order,
            'isDefault', NEW.is_default,
            'includeInDownload', 0,
            'lastModifiedAt', (SELECT last_modified_at FROM category WHERE id = NEW.id)
        )
    WHERE EXISTS (SELECT 1 FROM user u WHERE u.id = NEW.user_id);
END;

DROP TRIGGER category_sync_delete;
CREATE TRIGGER category_sync_delete BEFORE DELETE ON category
BEGIN
    INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
    SELECT
        OLD.user_id,
        'category',
        CAST(OLD.id AS TEXT),
        1,
        MAX(
            CAST(strftime('%s', 'now') AS INTEGER) * 1000 + CAST(substr(strftime('%f', 'now'), 4, 3) AS INTEGER),
            OLD.last_modified_at + 1
        ),
        json_object('categoryId', OLD.id, 'userId', OLD.user_id),
        json_object(
            'id', OLD.id,
            'userId', OLD.user_id,
            'name', OLD.name,
            'sortOrder', OLD.sort_order,
            'isDefault', OLD.is_default,
            'includeInDownload', 0,
            'lastModifiedAt', OLD.last_modified_at
        )
    WHERE EXISTS (SELECT 1 FROM user u WHERE u.id = OLD.user_id);
END;
