ALTER TABLE manga ADD COLUMN last_modified_at INTEGER NOT NULL DEFAULT 0;
ALTER TABLE manga_user ADD COLUMN last_modified_at INTEGER NOT NULL DEFAULT 0;
ALTER TABLE category ADD COLUMN last_modified_at INTEGER NOT NULL DEFAULT 0;
ALTER TABLE category_manga ADD COLUMN last_modified_at INTEGER NOT NULL DEFAULT 0;

CREATE TABLE sync_change (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL REFERENCES user (id) ON DELETE CASCADE,
    entity_kind TEXT NOT NULL,
    entity_key TEXT NOT NULL,
    deleted INTEGER NOT NULL DEFAULT 0,
    modified_at INTEGER NOT NULL,
    identity TEXT NOT NULL,
    snapshot TEXT NOT NULL
);
CREATE INDEX sync_change_user_sequence ON sync_change (user_id, sequence);
CREATE INDEX sync_change_user_entity_sequence ON sync_change (user_id, entity_kind, entity_key, sequence);
CREATE INDEX sync_change_entity_key ON sync_change (entity_kind, entity_key);

CREATE VIEW sync_manga_state AS
SELECT
    m.id AS manga_id,
    m.source_id,
    m.last_modified_at,
    json_object(
        'mangaId', m.id,
        'sourceId', m.source_id,
        'sourceKey', s.key,
        'mangaKey', m.key
    ) AS identity,
    json_object(
        'id', m.id,
        'sourceId', m.source_id,
        'key', m.key,
        'title', m.title,
        'authors', json(m.authors),
        'artists', json(m.artists),
        'description', m.description,
        'tags', json(m.tags),
        'status', m.status,
        'contentRating', m.content_rating,
        'viewer', m.viewer,
        'coverUrl', m.cover_url,
        'url', m.url,
        'initialized', m.initialized,
        'updateStrategy', m.update_strategy,
        'lastModifiedAt', m.last_modified_at
    ) AS snapshot
FROM manga m
JOIN source s ON s.id = m.source_id;

INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
SELECT
    mu.user_id,
    'manga',
    CAST(sm.manga_id AS TEXT),
    0,
    0,
    sm.identity,
    sm.snapshot
FROM manga_user mu
JOIN sync_manga_state sm ON sm.manga_id = mu.manga_id
JOIN user u ON u.id = mu.user_id
ORDER BY mu.user_id, sm.manga_id;

INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
SELECT
    mu.user_id,
    'manga_user',
    mu.manga_id || ':' || mu.user_id,
    0,
    0,
    json_set(sm.identity, '$.userId', mu.user_id),
    json_object(
        'mangaId', mu.manga_id,
        'userId', mu.user_id,
        'inLibrary', mu.in_library,
        'inLibraryAt', mu.in_library_at,
        'viewerFlags', mu.viewer_flags,
        'sortKey', mu.sort_key,
        'incognito', mu.incognito,
        'lastModifiedAt', mu.last_modified_at
    )
FROM manga_user mu
JOIN sync_manga_state sm ON sm.manga_id = mu.manga_id
JOIN user u ON u.id = mu.user_id
ORDER BY mu.user_id, mu.manga_id;

INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
SELECT
    c.user_id,
    'category',
    CAST(c.id AS TEXT),
    0,
    0,
    json_object('categoryId', c.id, 'userId', c.user_id),
    json_object(
        'id', c.id,
        'userId', c.user_id,
        'name', c.name,
        'sortOrder', c.sort_order,
        'isDefault', c.is_default,
        'includeInDownload', c.include_in_download,
        'lastModifiedAt', c.last_modified_at
    )
FROM category c
JOIN user u ON u.id = c.user_id
ORDER BY c.user_id, c.id;

INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
SELECT
    cm.user_id,
    'category_manga',
    cm.category_id || ':' || cm.manga_id || ':' || cm.user_id,
    0,
    0,
    json_set(sm.identity, '$.categoryId', cm.category_id, '$.userId', cm.user_id),
    json_object(
        'categoryId', cm.category_id,
        'mangaId', cm.manga_id,
        'userId', cm.user_id,
        'lastModifiedAt', cm.last_modified_at
    )
FROM category_manga cm
JOIN sync_manga_state sm ON sm.manga_id = cm.manga_id
JOIN user u ON u.id = cm.user_id
ORDER BY cm.user_id, cm.manga_id, cm.category_id;

CREATE TRIGGER manga_sync_insert AFTER INSERT ON manga
BEGIN
    UPDATE manga
    SET last_modified_at = CASE
        WHEN NEW.last_modified_at <> 0 THEN NEW.last_modified_at
        ELSE MAX(
            CAST(strftime('%s', 'now') AS INTEGER) * 1000 + CAST(substr(strftime('%f', 'now'), 4, 3) AS INTEGER),
            COALESCE((
                SELECT MAX(sc.modified_at) + 1 FROM sync_change sc
                WHERE sc.entity_kind = 'manga' AND sc.entity_key = CAST(NEW.id AS TEXT)
            ), 0)
        )
    END
    WHERE id = NEW.id;

    INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
    SELECT
        mu.user_id,
        'manga',
        CAST(NEW.id AS TEXT),
        0,
        sm.last_modified_at,
        sm.identity,
        sm.snapshot
    FROM sync_manga_state sm
    JOIN manga_user mu ON mu.manga_id = sm.manga_id
    JOIN user u ON u.id = mu.user_id
    WHERE sm.manga_id = NEW.id;
END;

CREATE TRIGGER manga_sync_update AFTER UPDATE ON manga
WHEN
    OLD.source_id IS NOT NEW.source_id OR
    OLD.key IS NOT NEW.key OR
    OLD.title IS NOT NEW.title OR
    OLD.authors IS NOT NEW.authors OR
    OLD.artists IS NOT NEW.artists OR
    OLD.description IS NOT NEW.description OR
    OLD.tags IS NOT NEW.tags OR
    OLD.status IS NOT NEW.status OR
    OLD.content_rating IS NOT NEW.content_rating OR
    OLD.viewer IS NOT NEW.viewer OR
    OLD.cover_url IS NOT NEW.cover_url OR
    OLD.url IS NOT NEW.url OR
    OLD.initialized IS NOT NEW.initialized OR
    OLD.update_strategy IS NOT NEW.update_strategy
BEGIN
    UPDATE manga
    SET last_modified_at = CASE
        WHEN NEW.last_modified_at <> 0 AND NEW.last_modified_at IS NOT OLD.last_modified_at THEN NEW.last_modified_at
        ELSE MAX(
            CAST(strftime('%s', 'now') AS INTEGER) * 1000 + CAST(substr(strftime('%f', 'now'), 4, 3) AS INTEGER),
            OLD.last_modified_at + 1
        )
    END
    WHERE id = NEW.id;

    INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
    SELECT
        mu.user_id,
        'manga',
        CAST(NEW.id AS TEXT),
        0,
        sm.last_modified_at,
        sm.identity,
        sm.snapshot
    FROM sync_manga_state sm
    JOIN manga_user mu ON mu.manga_id = sm.manga_id
    JOIN user u ON u.id = mu.user_id
    WHERE sm.manga_id = NEW.id;
END;

CREATE TRIGGER manga_sync_delete BEFORE DELETE ON manga
BEGIN
    INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
    SELECT
        mu.user_id,
        'manga',
        CAST(OLD.id AS TEXT),
        1,
        MAX(
            CAST(strftime('%s', 'now') AS INTEGER) * 1000 + CAST(substr(strftime('%f', 'now'), 4, 3) AS INTEGER),
            OLD.last_modified_at + 1
        ),
        sm.identity,
        sm.snapshot
    FROM sync_manga_state sm
    JOIN manga_user mu ON mu.manga_id = sm.manga_id
    JOIN user u ON u.id = mu.user_id
    WHERE sm.manga_id = OLD.id;
END;

-- The source row is already invisible to manga cascade triggers. Capturing its key here both preserves
-- the natural identity and makes the manga delete trigger a no-op for this cascade.
CREATE TRIGGER source_sync_delete BEFORE DELETE ON source
BEGIN
    INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
    SELECT
        mu.user_id,
        'manga',
        CAST(sm.manga_id AS TEXT),
        1,
        MAX(
            CAST(strftime('%s', 'now') AS INTEGER) * 1000 + CAST(substr(strftime('%f', 'now'), 4, 3) AS INTEGER),
            sm.last_modified_at + 1
        ),
        sm.identity,
        sm.snapshot
    FROM sync_manga_state sm
    JOIN manga_user mu ON mu.manga_id = sm.manga_id
    JOIN user u ON u.id = mu.user_id
    WHERE sm.source_id = OLD.id
    ORDER BY sm.manga_id, mu.user_id;
END;

CREATE TRIGGER manga_user_sync_insert AFTER INSERT ON manga_user
BEGIN
    UPDATE manga_user
    SET last_modified_at = CASE
        WHEN NEW.last_modified_at <> 0 THEN NEW.last_modified_at
        ELSE MAX(
            CAST(strftime('%s', 'now') AS INTEGER) * 1000 + CAST(substr(strftime('%f', 'now'), 4, 3) AS INTEGER),
            COALESCE((
                SELECT MAX(sc.modified_at) + 1 FROM sync_change sc
                WHERE sc.user_id = NEW.user_id AND sc.entity_kind = 'manga_user'
                  AND sc.entity_key = NEW.manga_id || ':' || NEW.user_id
            ), 0)
        )
    END
    WHERE manga_id = NEW.manga_id AND user_id = NEW.user_id;

    INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
    SELECT
        NEW.user_id,
        'manga',
        CAST(NEW.manga_id AS TEXT),
        0,
        sm.last_modified_at,
        sm.identity,
        sm.snapshot
    FROM sync_manga_state sm
    JOIN user u ON u.id = NEW.user_id
    WHERE sm.manga_id = NEW.manga_id;

    INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
    SELECT
        NEW.user_id,
        'manga_user',
        NEW.manga_id || ':' || NEW.user_id,
        0,
        (SELECT last_modified_at FROM manga_user WHERE manga_id = NEW.manga_id AND user_id = NEW.user_id),
        json_set(sm.identity, '$.userId', NEW.user_id),
        json_object(
            'mangaId', NEW.manga_id,
            'userId', NEW.user_id,
            'inLibrary', NEW.in_library,
            'inLibraryAt', NEW.in_library_at,
            'viewerFlags', NEW.viewer_flags,
            'sortKey', NEW.sort_key,
            'incognito', NEW.incognito,
            'lastModifiedAt', (SELECT last_modified_at FROM manga_user WHERE manga_id = NEW.manga_id AND user_id = NEW.user_id)
        )
    FROM sync_manga_state sm
    JOIN user u ON u.id = NEW.user_id
    WHERE sm.manga_id = NEW.manga_id;
END;

CREATE TRIGGER manga_user_sync_update AFTER UPDATE ON manga_user
WHEN
    OLD.manga_id IS NOT NEW.manga_id OR
    OLD.user_id IS NOT NEW.user_id OR
    OLD.in_library IS NOT NEW.in_library OR
    OLD.in_library_at IS NOT NEW.in_library_at OR
    OLD.viewer_flags IS NOT NEW.viewer_flags OR
    OLD.sort_key IS NOT NEW.sort_key OR
    OLD.incognito IS NOT NEW.incognito
BEGIN
    UPDATE manga_user
    SET last_modified_at = CASE
        WHEN NEW.last_modified_at <> 0 AND NEW.last_modified_at IS NOT OLD.last_modified_at THEN NEW.last_modified_at
        ELSE MAX(
            CAST(strftime('%s', 'now') AS INTEGER) * 1000 + CAST(substr(strftime('%f', 'now'), 4, 3) AS INTEGER),
            OLD.last_modified_at + 1,
            COALESCE((
                SELECT MAX(sc.modified_at) + 1 FROM sync_change sc
                WHERE sc.user_id = NEW.user_id AND sc.entity_kind = 'manga_user'
                  AND sc.entity_key = NEW.manga_id || ':' || NEW.user_id
            ), 0)
        )
    END
    WHERE manga_id = NEW.manga_id AND user_id = NEW.user_id;

    INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
    SELECT
        OLD.user_id,
        'manga',
        CAST(OLD.manga_id AS TEXT),
        0,
        sm.last_modified_at,
        sm.identity,
        sm.snapshot
    FROM sync_manga_state sm
    JOIN user u ON u.id = OLD.user_id
    WHERE sm.manga_id = OLD.manga_id
      AND (OLD.manga_id IS NOT NEW.manga_id OR OLD.user_id IS NOT NEW.user_id);

    INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
    SELECT
        OLD.user_id,
        'manga_user',
        OLD.manga_id || ':' || OLD.user_id,
        1,
        (SELECT last_modified_at FROM manga_user WHERE manga_id = NEW.manga_id AND user_id = NEW.user_id),
        json_set(sm.identity, '$.userId', OLD.user_id),
        json_object(
            'mangaId', OLD.manga_id,
            'userId', OLD.user_id,
            'inLibrary', OLD.in_library,
            'inLibraryAt', OLD.in_library_at,
            'viewerFlags', OLD.viewer_flags,
            'sortKey', OLD.sort_key,
            'incognito', OLD.incognito,
            'lastModifiedAt', OLD.last_modified_at
        )
    FROM sync_manga_state sm
    JOIN user u ON u.id = OLD.user_id
    WHERE sm.manga_id = OLD.manga_id
      AND (OLD.manga_id IS NOT NEW.manga_id OR OLD.user_id IS NOT NEW.user_id);

    INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
    SELECT
        NEW.user_id,
        'manga',
        CAST(NEW.manga_id AS TEXT),
        0,
        sm.last_modified_at,
        sm.identity,
        sm.snapshot
    FROM sync_manga_state sm
    JOIN user u ON u.id = NEW.user_id
    WHERE sm.manga_id = NEW.manga_id;

    INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
    SELECT
        NEW.user_id,
        'manga_user',
        NEW.manga_id || ':' || NEW.user_id,
        0,
        (SELECT last_modified_at FROM manga_user WHERE manga_id = NEW.manga_id AND user_id = NEW.user_id),
        json_set(sm.identity, '$.userId', NEW.user_id),
        json_object(
            'mangaId', NEW.manga_id,
            'userId', NEW.user_id,
            'inLibrary', NEW.in_library,
            'inLibraryAt', NEW.in_library_at,
            'viewerFlags', NEW.viewer_flags,
            'sortKey', NEW.sort_key,
            'incognito', NEW.incognito,
            'lastModifiedAt', (SELECT last_modified_at FROM manga_user WHERE manga_id = NEW.manga_id AND user_id = NEW.user_id)
        )
    FROM sync_manga_state sm
    JOIN user u ON u.id = NEW.user_id
    WHERE sm.manga_id = NEW.manga_id;
END;

CREATE TRIGGER manga_user_sync_delete AFTER DELETE ON manga_user
BEGIN
    INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
    SELECT
        OLD.user_id,
        'manga',
        CAST(OLD.manga_id AS TEXT),
        0,
        sm.last_modified_at,
        sm.identity,
        sm.snapshot
    FROM sync_manga_state sm
    JOIN user u ON u.id = OLD.user_id
    WHERE sm.manga_id = OLD.manga_id;

    INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
    SELECT
        OLD.user_id,
        'manga_user',
        OLD.manga_id || ':' || OLD.user_id,
        1,
        MAX(
            CAST(strftime('%s', 'now') AS INTEGER) * 1000 + CAST(substr(strftime('%f', 'now'), 4, 3) AS INTEGER),
            OLD.last_modified_at + 1
        ),
        json_set(
            COALESCE(
                (SELECT sm.identity FROM sync_manga_state sm WHERE sm.manga_id = OLD.manga_id),
                (SELECT sc.identity FROM sync_change sc
                 WHERE sc.user_id = OLD.user_id AND sc.entity_kind = 'manga'
                   AND sc.entity_key = CAST(OLD.manga_id AS TEXT)
                 ORDER BY sc.sequence DESC LIMIT 1)
            ),
            '$.userId', OLD.user_id
        ),
        json_object(
            'mangaId', OLD.manga_id,
            'userId', OLD.user_id,
            'inLibrary', OLD.in_library,
            'inLibraryAt', OLD.in_library_at,
            'viewerFlags', OLD.viewer_flags,
            'sortKey', OLD.sort_key,
            'incognito', OLD.incognito,
            'lastModifiedAt', OLD.last_modified_at
        )
    WHERE EXISTS (SELECT 1 FROM user u WHERE u.id = OLD.user_id);
END;

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
            'includeInDownload', NEW.include_in_download,
            'lastModifiedAt', (SELECT last_modified_at FROM category WHERE id = NEW.id)
        )
    WHERE EXISTS (SELECT 1 FROM user u WHERE u.id = NEW.user_id);
END;

CREATE TRIGGER category_sync_update AFTER UPDATE ON category
WHEN
    OLD.user_id IS NOT NEW.user_id OR
    OLD.name IS NOT NEW.name OR
    OLD.sort_order IS NOT NEW.sort_order OR
    OLD.is_default IS NOT NEW.is_default OR
    OLD.include_in_download IS NOT NEW.include_in_download
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
            'includeInDownload', OLD.include_in_download,
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
            'includeInDownload', NEW.include_in_download,
            'lastModifiedAt', (SELECT last_modified_at FROM category WHERE id = NEW.id)
        )
    WHERE EXISTS (SELECT 1 FROM user u WHERE u.id = NEW.user_id);
END;

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
            'includeInDownload', OLD.include_in_download,
            'lastModifiedAt', OLD.last_modified_at
        )
    WHERE EXISTS (SELECT 1 FROM user u WHERE u.id = OLD.user_id);
END;

CREATE TRIGGER category_manga_sync_insert AFTER INSERT ON category_manga
BEGIN
    UPDATE category_manga
    SET last_modified_at = CASE
        WHEN NEW.last_modified_at <> 0 THEN NEW.last_modified_at
        ELSE MAX(
            CAST(strftime('%s', 'now') AS INTEGER) * 1000 + CAST(substr(strftime('%f', 'now'), 4, 3) AS INTEGER),
            COALESCE((
                SELECT MAX(sc.modified_at) + 1 FROM sync_change sc
                WHERE sc.user_id = NEW.user_id AND sc.entity_kind = 'category_manga'
                  AND sc.entity_key = NEW.category_id || ':' || NEW.manga_id || ':' || NEW.user_id
            ), 0)
        )
    END
    WHERE category_id = NEW.category_id AND manga_id = NEW.manga_id AND user_id = NEW.user_id;

    INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
    SELECT
        NEW.user_id,
        'manga',
        CAST(NEW.manga_id AS TEXT),
        0,
        sm.last_modified_at,
        sm.identity,
        sm.snapshot
    FROM sync_manga_state sm
    JOIN user u ON u.id = NEW.user_id
    WHERE sm.manga_id = NEW.manga_id;

    INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
    SELECT
        NEW.user_id,
        'category_manga',
        NEW.category_id || ':' || NEW.manga_id || ':' || NEW.user_id,
        0,
        (SELECT last_modified_at FROM category_manga
         WHERE category_id = NEW.category_id AND manga_id = NEW.manga_id AND user_id = NEW.user_id),
        json_set(sm.identity, '$.categoryId', NEW.category_id, '$.userId', NEW.user_id),
        json_object(
            'categoryId', NEW.category_id,
            'mangaId', NEW.manga_id,
            'userId', NEW.user_id,
            'lastModifiedAt', (SELECT last_modified_at FROM category_manga
             WHERE category_id = NEW.category_id AND manga_id = NEW.manga_id AND user_id = NEW.user_id)
        )
    FROM sync_manga_state sm
    JOIN user u ON u.id = NEW.user_id
    WHERE sm.manga_id = NEW.manga_id;
END;

CREATE TRIGGER category_manga_sync_update AFTER UPDATE ON category_manga
WHEN
    OLD.category_id IS NOT NEW.category_id OR
    OLD.manga_id IS NOT NEW.manga_id OR
    OLD.user_id IS NOT NEW.user_id
BEGIN
    UPDATE category_manga
    SET last_modified_at = CASE
        WHEN NEW.last_modified_at <> 0 AND NEW.last_modified_at IS NOT OLD.last_modified_at THEN NEW.last_modified_at
        ELSE MAX(
            CAST(strftime('%s', 'now') AS INTEGER) * 1000 + CAST(substr(strftime('%f', 'now'), 4, 3) AS INTEGER),
            OLD.last_modified_at + 1,
            COALESCE((
                SELECT MAX(sc.modified_at) + 1 FROM sync_change sc
                WHERE sc.user_id = NEW.user_id AND sc.entity_kind = 'category_manga'
                  AND sc.entity_key = NEW.category_id || ':' || NEW.manga_id || ':' || NEW.user_id
            ), 0)
        )
    END
    WHERE category_id = NEW.category_id AND manga_id = NEW.manga_id AND user_id = NEW.user_id;

    INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
    SELECT
        OLD.user_id,
        'manga',
        CAST(OLD.manga_id AS TEXT),
        0,
        sm.last_modified_at,
        sm.identity,
        sm.snapshot
    FROM sync_manga_state sm
    JOIN user u ON u.id = OLD.user_id
    WHERE sm.manga_id = OLD.manga_id;

    INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
    SELECT
        OLD.user_id,
        'category_manga',
        OLD.category_id || ':' || OLD.manga_id || ':' || OLD.user_id,
        1,
        (SELECT last_modified_at FROM category_manga
         WHERE category_id = NEW.category_id AND manga_id = NEW.manga_id AND user_id = NEW.user_id),
        json_set(sm.identity, '$.categoryId', OLD.category_id, '$.userId', OLD.user_id),
        json_object(
            'categoryId', OLD.category_id,
            'mangaId', OLD.manga_id,
            'userId', OLD.user_id,
            'lastModifiedAt', OLD.last_modified_at
        )
    FROM sync_manga_state sm
    JOIN user u ON u.id = OLD.user_id
    WHERE sm.manga_id = OLD.manga_id;

    INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
    SELECT
        NEW.user_id,
        'manga',
        CAST(NEW.manga_id AS TEXT),
        0,
        sm.last_modified_at,
        sm.identity,
        sm.snapshot
    FROM sync_manga_state sm
    JOIN user u ON u.id = NEW.user_id
    WHERE sm.manga_id = NEW.manga_id;

    INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
    SELECT
        NEW.user_id,
        'category_manga',
        NEW.category_id || ':' || NEW.manga_id || ':' || NEW.user_id,
        0,
        (SELECT last_modified_at FROM category_manga
         WHERE category_id = NEW.category_id AND manga_id = NEW.manga_id AND user_id = NEW.user_id),
        json_set(sm.identity, '$.categoryId', NEW.category_id, '$.userId', NEW.user_id),
        json_object(
            'categoryId', NEW.category_id,
            'mangaId', NEW.manga_id,
            'userId', NEW.user_id,
            'lastModifiedAt', (SELECT last_modified_at FROM category_manga
             WHERE category_id = NEW.category_id AND manga_id = NEW.manga_id AND user_id = NEW.user_id)
        )
    FROM sync_manga_state sm
    JOIN user u ON u.id = NEW.user_id
    WHERE sm.manga_id = NEW.manga_id;
END;

CREATE TRIGGER category_manga_sync_delete AFTER DELETE ON category_manga
BEGIN
    INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
    SELECT
        OLD.user_id,
        'manga',
        CAST(OLD.manga_id AS TEXT),
        0,
        sm.last_modified_at,
        sm.identity,
        sm.snapshot
    FROM sync_manga_state sm
    JOIN user u ON u.id = OLD.user_id
    WHERE sm.manga_id = OLD.manga_id;

    INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
    SELECT
        OLD.user_id,
        'category_manga',
        OLD.category_id || ':' || OLD.manga_id || ':' || OLD.user_id,
        1,
        MAX(
            CAST(strftime('%s', 'now') AS INTEGER) * 1000 + CAST(substr(strftime('%f', 'now'), 4, 3) AS INTEGER),
            OLD.last_modified_at + 1
        ),
        json_set(
            COALESCE(
                (SELECT sm.identity FROM sync_manga_state sm WHERE sm.manga_id = OLD.manga_id),
                (SELECT sc.identity FROM sync_change sc
                 WHERE sc.user_id = OLD.user_id AND sc.entity_kind = 'manga'
                   AND sc.entity_key = CAST(OLD.manga_id AS TEXT)
                 ORDER BY sc.sequence DESC LIMIT 1)
            ),
            '$.categoryId', OLD.category_id,
            '$.userId', OLD.user_id
        ),
        json_object(
            'categoryId', OLD.category_id,
            'mangaId', OLD.manga_id,
            'userId', OLD.user_id,
            'lastModifiedAt', OLD.last_modified_at
        )
    WHERE EXISTS (SELECT 1 FROM user u WHERE u.id = OLD.user_id);
END;
