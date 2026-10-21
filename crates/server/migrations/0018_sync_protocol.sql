ALTER TABLE chapter ADD COLUMN last_modified_at INTEGER NOT NULL DEFAULT 0;
ALTER TABLE chapter_user ADD COLUMN last_modified_at INTEGER NOT NULL DEFAULT 0;

ALTER TABLE sync_change ADD COLUMN origin TEXT NOT NULL DEFAULT '';
ALTER TABLE sync_change ADD COLUMN origin_user_id INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sync_change ADD COLUMN origin_entity_key TEXT NOT NULL DEFAULT '';
ALTER TABLE sync_change ADD COLUMN origin_sequence INTEGER NOT NULL DEFAULT 0;

INSERT OR IGNORE INTO setting (key, value) VALUES ('sync.instance_id', hex(randomblob(16)));

UPDATE sync_change
SET origin = (SELECT value FROM setting WHERE key = 'sync.instance_id'),
    origin_user_id = user_id,
    origin_entity_key = entity_key,
    origin_sequence = sequence;

CREATE INDEX sync_change_origin_entity_sequence ON sync_change
    (origin, origin_user_id, entity_kind, origin_entity_key, origin_sequence);

CREATE TRIGGER sync_change_origin_insert AFTER INSERT ON sync_change
WHEN NEW.origin_sequence = 0
BEGIN
    UPDATE sync_change
    SET origin = (SELECT value FROM setting WHERE key = 'sync.instance_id'),
        origin_user_id = NEW.user_id,
        origin_entity_key = NEW.entity_key,
        origin_sequence = NEW.sequence
    WHERE sequence = NEW.sequence;
END;

CREATE TABLE sync_origin_state (
    origin TEXT NOT NULL,
    origin_user_id INTEGER NOT NULL,
    user_id INTEGER NOT NULL REFERENCES user (id) ON DELETE CASCADE,
    entity_kind TEXT NOT NULL,
    origin_entity_key TEXT NOT NULL,
    origin_sequence INTEGER NOT NULL,
    PRIMARY KEY (origin, origin_user_id, user_id, entity_kind, origin_entity_key)
);

CREATE TABLE sync_checkpoint (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    base_sequence INTEGER NOT NULL DEFAULT 0
);
INSERT INTO sync_checkpoint (id, base_sequence) VALUES (1, 0);

CREATE TABLE sync_chapter_state (
    chapter_id INTEGER PRIMARY KEY,
    manga_id INTEGER NOT NULL,
    source_id INTEGER NOT NULL,
    source_key TEXT NOT NULL,
    manga_key TEXT NOT NULL,
    chapter_key TEXT NOT NULL,
    identity TEXT NOT NULL,
    snapshot TEXT NOT NULL,
    UNIQUE (source_key, manga_key, chapter_key)
);
CREATE INDEX sync_chapter_state_manga ON sync_chapter_state (manga_id, chapter_id);
CREATE INDEX sync_chapter_state_source ON sync_chapter_state (source_id, manga_id, chapter_id);

INSERT INTO sync_chapter_state
    (chapter_id, manga_id, source_id, source_key, manga_key, chapter_key, identity, snapshot)
SELECT
    c.id,
    m.id,
    s.id,
    s.key,
    m.key,
    c.key,
    json_object(
        'chapterId', c.id,
        'mangaId', m.id,
        'sourceId', s.id,
        'sourceKey', s.key,
        'mangaKey', m.key,
        'chapterKey', c.key
    ),
    json_object(
        'id', c.id,
        'mangaId', c.manga_id,
        'key', c.key,
        'title', c.title,
        'number', c.number,
        'volume', c.volume,
        'scanlators', json(c.scanlators),
        'language', c.language,
        'thumbnail', c.thumbnail,
        'url', c.url,
        'dateUploaded', c.date_uploaded,
        'sourceOrder', c.source_order,
        'pageCount', c.page_count,
        'locked', c.locked,
        'lastModifiedAt', c.last_modified_at
    )
FROM chapter c
JOIN manga m ON m.id = c.manga_id
JOIN source s ON s.id = m.source_id;

INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
SELECT DISTINCT
    cu.user_id,
    'manga',
    CAST(sm.manga_id AS TEXT),
    0,
    sm.last_modified_at,
    sm.identity,
    sm.snapshot
FROM chapter_user cu
JOIN chapter c ON c.id = cu.chapter_id
JOIN sync_manga_state sm ON sm.manga_id = c.manga_id
JOIN user u ON u.id = cu.user_id
WHERE NOT EXISTS (
    SELECT 1 FROM manga_user mu WHERE mu.manga_id = c.manga_id AND mu.user_id = cu.user_id
)
ORDER BY cu.user_id, sm.manga_id;

INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
SELECT
    fans.user_id,
    'chapter',
    CAST(scs.chapter_id AS TEXT),
    0,
    0,
    scs.identity,
    scs.snapshot
FROM sync_chapter_state scs
JOIN (
    SELECT c.id AS chapter_id, mu.user_id
    FROM chapter c
    JOIN manga_user mu ON mu.manga_id = c.manga_id
    UNION
    SELECT cu.chapter_id, cu.user_id FROM chapter_user cu
) fans ON fans.chapter_id = scs.chapter_id
JOIN user u ON u.id = fans.user_id
ORDER BY fans.user_id, scs.manga_id, scs.chapter_id;

INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
SELECT
    cu.user_id,
    'chapter_user',
    cu.chapter_id || ':' || cu.user_id,
    0,
    0,
    json_set(scs.identity, '$.userId', cu.user_id),
    json_object(
        'chapterId', cu.chapter_id,
        'userId', cu.user_id,
        'lastPageRead', cu.last_page_read,
        'isRead', cu.is_read,
        'isBookmarked', cu.is_bookmarked,
        'lastReadAt', cu.last_read_at,
        'lastModifiedAt', cu.last_modified_at
    )
FROM chapter_user cu
JOIN sync_chapter_state scs ON scs.chapter_id = cu.chapter_id
JOIN user u ON u.id = cu.user_id
ORDER BY cu.user_id, cu.chapter_id;

CREATE TRIGGER manga_sync_chapter_state_update AFTER UPDATE OF source_id, key ON manga
BEGIN
    UPDATE sync_chapter_state
    SET source_id = NEW.source_id,
        source_key = (SELECT key FROM source WHERE id = NEW.source_id),
        manga_key = NEW.key,
        identity = json_set(
            identity,
            '$.sourceId', NEW.source_id,
            '$.sourceKey', (SELECT key FROM source WHERE id = NEW.source_id),
            '$.mangaKey', NEW.key
        )
    WHERE manga_id = NEW.id;
END;

CREATE TRIGGER source_sync_chapter_state_update AFTER UPDATE OF key ON source
BEGIN
    UPDATE sync_chapter_state
    SET source_key = NEW.key,
        identity = json_set(identity, '$.sourceKey', NEW.key)
    WHERE source_id = NEW.id;
END;

CREATE TRIGGER chapter_sync_state_version_update AFTER UPDATE OF last_modified_at ON chapter
WHEN OLD.last_modified_at IS NOT NEW.last_modified_at
BEGIN
    UPDATE sync_chapter_state
    SET snapshot = json_set(snapshot, '$.lastModifiedAt', NEW.last_modified_at)
    WHERE chapter_id = NEW.id;
END;

CREATE TRIGGER chapter_sync_insert AFTER INSERT ON chapter
BEGIN
    UPDATE chapter
    SET last_modified_at = CASE
        WHEN NEW.last_modified_at <> 0 THEN NEW.last_modified_at
        ELSE MAX(
            CAST(strftime('%s', 'now') AS INTEGER) * 1000 + CAST(substr(strftime('%f', 'now'), 4, 3) AS INTEGER),
            COALESCE((
                SELECT MAX(sc.modified_at) + 1 FROM sync_change sc
                WHERE sc.entity_kind = 'chapter' AND sc.entity_key = CAST(NEW.id AS TEXT)
            ), 0)
        )
    END
    WHERE id = NEW.id;

    INSERT INTO sync_chapter_state
        (chapter_id, manga_id, source_id, source_key, manga_key, chapter_key, identity, snapshot)
    SELECT
        c.id,
        m.id,
        s.id,
        s.key,
        m.key,
        c.key,
        json_object(
            'chapterId', c.id,
            'mangaId', m.id,
            'sourceId', s.id,
            'sourceKey', s.key,
            'mangaKey', m.key,
            'chapterKey', c.key
        ),
        json_object(
            'id', c.id,
            'mangaId', c.manga_id,
            'key', c.key,
            'title', c.title,
            'number', c.number,
            'volume', c.volume,
            'scanlators', json(c.scanlators),
            'language', c.language,
            'thumbnail', c.thumbnail,
            'url', c.url,
            'dateUploaded', c.date_uploaded,
            'sourceOrder', c.source_order,
            'pageCount', c.page_count,
            'locked', c.locked,
            'lastModifiedAt', c.last_modified_at
        )
    FROM chapter c
    JOIN manga m ON m.id = c.manga_id
    JOIN source s ON s.id = m.source_id
    WHERE c.id = NEW.id;

    INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
    SELECT
        fans.user_id,
        'manga',
        CAST(sm.manga_id AS TEXT),
        0,
        sm.last_modified_at,
        sm.identity,
        sm.snapshot
    FROM sync_manga_state sm
    JOIN (
        SELECT mu.user_id FROM manga_user mu WHERE mu.manga_id = NEW.manga_id
        UNION
        SELECT cu.user_id FROM chapter_user cu WHERE cu.chapter_id = NEW.id
    ) fans ON 1 = 1
    JOIN user u ON u.id = fans.user_id
    WHERE sm.manga_id = NEW.manga_id
    ORDER BY fans.user_id;

    INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
    SELECT
        fans.user_id,
        'chapter',
        CAST(scs.chapter_id AS TEXT),
        0,
        CAST(json_extract(scs.snapshot, '$.lastModifiedAt') AS INTEGER),
        scs.identity,
        scs.snapshot
    FROM sync_chapter_state scs
    JOIN (
        SELECT mu.user_id FROM manga_user mu WHERE mu.manga_id = NEW.manga_id
        UNION
        SELECT cu.user_id FROM chapter_user cu WHERE cu.chapter_id = NEW.id
    ) fans ON 1 = 1
    JOIN user u ON u.id = fans.user_id
    WHERE scs.chapter_id = NEW.id
    ORDER BY fans.user_id;
END;

CREATE TRIGGER chapter_sync_update AFTER UPDATE ON chapter
WHEN
    OLD.manga_id IS NOT NEW.manga_id OR
    OLD.key IS NOT NEW.key OR
    OLD.title IS NOT NEW.title OR
    OLD.number IS NOT NEW.number OR
    OLD.volume IS NOT NEW.volume OR
    OLD.scanlators IS NOT NEW.scanlators OR
    OLD.language IS NOT NEW.language OR
    OLD.thumbnail IS NOT NEW.thumbnail OR
    OLD.url IS NOT NEW.url OR
    OLD.date_uploaded IS NOT NEW.date_uploaded OR
    OLD.source_order IS NOT NEW.source_order OR
    OLD.page_count IS NOT NEW.page_count OR
    OLD.locked IS NOT NEW.locked
BEGIN
    UPDATE chapter
    SET last_modified_at = CASE
        WHEN NEW.last_modified_at <> 0 AND NEW.last_modified_at IS NOT OLD.last_modified_at THEN NEW.last_modified_at
        ELSE MAX(
            CAST(strftime('%s', 'now') AS INTEGER) * 1000 + CAST(substr(strftime('%f', 'now'), 4, 3) AS INTEGER),
            OLD.last_modified_at + 1,
            COALESCE((
                SELECT MAX(sc.modified_at) + 1 FROM sync_change sc
                WHERE sc.entity_kind = 'chapter' AND sc.entity_key = CAST(NEW.id AS TEXT)
            ), 0)
        )
    END
    WHERE id = NEW.id;

    INSERT INTO sync_chapter_state
        (chapter_id, manga_id, source_id, source_key, manga_key, chapter_key, identity, snapshot)
    SELECT
        c.id,
        m.id,
        s.id,
        s.key,
        m.key,
        c.key,
        json_object(
            'chapterId', c.id,
            'mangaId', m.id,
            'sourceId', s.id,
            'sourceKey', s.key,
            'mangaKey', m.key,
            'chapterKey', c.key
        ),
        json_object(
            'id', c.id,
            'mangaId', c.manga_id,
            'key', c.key,
            'title', c.title,
            'number', c.number,
            'volume', c.volume,
            'scanlators', json(c.scanlators),
            'language', c.language,
            'thumbnail', c.thumbnail,
            'url', c.url,
            'dateUploaded', c.date_uploaded,
            'sourceOrder', c.source_order,
            'pageCount', c.page_count,
            'locked', c.locked,
            'lastModifiedAt', c.last_modified_at
        )
    FROM chapter c
    JOIN manga m ON m.id = c.manga_id
    JOIN source s ON s.id = m.source_id
    WHERE c.id = NEW.id
    ON CONFLICT (chapter_id) DO UPDATE SET
        manga_id = excluded.manga_id,
        source_id = excluded.source_id,
        source_key = excluded.source_key,
        manga_key = excluded.manga_key,
        chapter_key = excluded.chapter_key,
        identity = excluded.identity,
        snapshot = excluded.snapshot;

    INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
    SELECT
        fans.user_id,
        'manga',
        CAST(sm.manga_id AS TEXT),
        0,
        sm.last_modified_at,
        sm.identity,
        sm.snapshot
    FROM sync_manga_state sm
    JOIN (
        SELECT mu.user_id FROM manga_user mu WHERE mu.manga_id = OLD.manga_id
        UNION
        SELECT cu.user_id FROM chapter_user cu WHERE cu.chapter_id = OLD.id
    ) fans ON 1 = 1
    JOIN user u ON u.id = fans.user_id
    WHERE sm.manga_id = OLD.manga_id
      AND OLD.manga_id IS NOT NEW.manga_id
    ORDER BY fans.user_id;

    INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
    SELECT
        fans.user_id,
        'manga',
        CAST(sm.manga_id AS TEXT),
        0,
        sm.last_modified_at,
        sm.identity,
        sm.snapshot
    FROM sync_manga_state sm
    JOIN (
        SELECT mu.user_id FROM manga_user mu WHERE mu.manga_id = NEW.manga_id
        UNION
        SELECT cu.user_id FROM chapter_user cu WHERE cu.chapter_id = NEW.id
    ) fans ON 1 = 1
    JOIN user u ON u.id = fans.user_id
    WHERE sm.manga_id = NEW.manga_id
    ORDER BY fans.user_id;

    INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
    SELECT
        fans.user_id,
        'chapter',
        CAST(scs.chapter_id AS TEXT),
        0,
        CAST(json_extract(scs.snapshot, '$.lastModifiedAt') AS INTEGER),
        scs.identity,
        scs.snapshot
    FROM sync_chapter_state scs
    JOIN (
        SELECT mu.user_id FROM manga_user mu WHERE mu.manga_id = NEW.manga_id
        UNION
        SELECT cu.user_id FROM chapter_user cu WHERE cu.chapter_id = NEW.id
        UNION
        SELECT mu.user_id FROM manga_user mu WHERE mu.manga_id = OLD.manga_id
        UNION
        SELECT cu.user_id FROM chapter_user cu WHERE cu.chapter_id = OLD.id
    ) fans ON 1 = 1
    JOIN user u ON u.id = fans.user_id
    WHERE scs.chapter_id = NEW.id
    ORDER BY fans.user_id;
END;

CREATE TRIGGER chapter_sync_delete BEFORE DELETE ON chapter
WHEN EXISTS (SELECT 1 FROM sync_chapter_state scs WHERE scs.chapter_id = OLD.id)
BEGIN
    INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
    SELECT
        fans.user_id,
        'manga',
        CAST(sm.manga_id AS TEXT),
        0,
        sm.last_modified_at,
        sm.identity,
        sm.snapshot
    FROM sync_manga_state sm
    JOIN (
        SELECT mu.user_id FROM manga_user mu WHERE mu.manga_id = OLD.manga_id
        UNION
        SELECT cu.user_id FROM chapter_user cu WHERE cu.chapter_id = OLD.id
    ) fans ON 1 = 1
    JOIN user u ON u.id = fans.user_id
    WHERE sm.manga_id = OLD.manga_id
    ORDER BY fans.user_id;

    INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
    SELECT
        fans.user_id,
        'chapter',
        CAST(scs.chapter_id AS TEXT),
        1,
        MAX(
            CAST(strftime('%s', 'now') AS INTEGER) * 1000 + CAST(substr(strftime('%f', 'now'), 4, 3) AS INTEGER),
            CAST(json_extract(scs.snapshot, '$.lastModifiedAt') AS INTEGER) + 1
        ),
        scs.identity,
        scs.snapshot
    FROM sync_chapter_state scs
    JOIN (
        SELECT mu.user_id FROM manga_user mu WHERE mu.manga_id = OLD.manga_id
        UNION
        SELECT cu.user_id FROM chapter_user cu WHERE cu.chapter_id = OLD.id
    ) fans ON 1 = 1
    JOIN user u ON u.id = fans.user_id
    WHERE scs.chapter_id = OLD.id
    ORDER BY fans.user_id;

    INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
    SELECT
        cu.user_id,
        'chapter_user',
        cu.chapter_id || ':' || cu.user_id,
        1,
        MAX(
            CAST(strftime('%s', 'now') AS INTEGER) * 1000 + CAST(substr(strftime('%f', 'now'), 4, 3) AS INTEGER),
            cu.last_modified_at + 1
        ),
        json_set(scs.identity, '$.userId', cu.user_id),
        json_object(
            'chapterId', cu.chapter_id,
            'userId', cu.user_id,
            'lastPageRead', cu.last_page_read,
            'isRead', cu.is_read,
            'isBookmarked', cu.is_bookmarked,
            'lastReadAt', cu.last_read_at,
            'lastModifiedAt', cu.last_modified_at
        )
    FROM chapter_user cu
    JOIN sync_chapter_state scs ON scs.chapter_id = cu.chapter_id
    JOIN user u ON u.id = cu.user_id
    WHERE cu.chapter_id = OLD.id
    ORDER BY cu.user_id;

    DELETE FROM sync_chapter_state WHERE chapter_id = OLD.id;
END;

CREATE TRIGGER chapter_user_sync_insert AFTER INSERT ON chapter_user
WHEN
    NEW.is_downloaded = 0 OR
    NEW.last_page_read <> 0 OR
    NEW.is_read <> 0 OR
    NEW.is_bookmarked <> 0 OR
    NEW.last_read_at IS NOT NULL
BEGIN
    UPDATE chapter_user
    SET last_modified_at = CASE
        WHEN NEW.last_modified_at <> 0 THEN NEW.last_modified_at
        ELSE MAX(
            CAST(strftime('%s', 'now') AS INTEGER) * 1000 + CAST(substr(strftime('%f', 'now'), 4, 3) AS INTEGER),
            COALESCE((
                SELECT MAX(sc.modified_at) + 1 FROM sync_change sc
                WHERE sc.user_id = NEW.user_id AND sc.entity_kind = 'chapter_user'
                  AND sc.entity_key = NEW.chapter_id || ':' || NEW.user_id
            ), 0)
        )
    END
    WHERE chapter_id = NEW.chapter_id AND user_id = NEW.user_id;

    INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
    SELECT
        NEW.user_id,
        'manga',
        CAST(sm.manga_id AS TEXT),
        0,
        sm.last_modified_at,
        sm.identity,
        sm.snapshot
    FROM sync_chapter_state scs
    JOIN sync_manga_state sm ON sm.manga_id = scs.manga_id
    JOIN user u ON u.id = NEW.user_id
    WHERE scs.chapter_id = NEW.chapter_id;

    INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
    SELECT
        NEW.user_id,
        'chapter',
        CAST(scs.chapter_id AS TEXT),
        0,
        CAST(json_extract(scs.snapshot, '$.lastModifiedAt') AS INTEGER),
        scs.identity,
        scs.snapshot
    FROM sync_chapter_state scs
    JOIN user u ON u.id = NEW.user_id
    WHERE scs.chapter_id = NEW.chapter_id;

    INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
    SELECT
        NEW.user_id,
        'chapter_user',
        NEW.chapter_id || ':' || NEW.user_id,
        0,
        (SELECT last_modified_at FROM chapter_user WHERE chapter_id = NEW.chapter_id AND user_id = NEW.user_id),
        json_set(scs.identity, '$.userId', NEW.user_id),
        json_object(
            'chapterId', NEW.chapter_id,
            'userId', NEW.user_id,
            'lastPageRead', NEW.last_page_read,
            'isRead', NEW.is_read,
            'isBookmarked', NEW.is_bookmarked,
            'lastReadAt', NEW.last_read_at,
            'lastModifiedAt', (SELECT last_modified_at FROM chapter_user WHERE chapter_id = NEW.chapter_id AND user_id = NEW.user_id)
        )
    FROM sync_chapter_state scs
    JOIN user u ON u.id = NEW.user_id
    WHERE scs.chapter_id = NEW.chapter_id;
END;

CREATE TRIGGER chapter_user_sync_update AFTER UPDATE ON chapter_user
WHEN
    OLD.chapter_id IS NOT NEW.chapter_id OR
    OLD.user_id IS NOT NEW.user_id OR
    OLD.last_page_read IS NOT NEW.last_page_read OR
    OLD.is_read IS NOT NEW.is_read OR
    OLD.is_bookmarked IS NOT NEW.is_bookmarked OR
    OLD.last_read_at IS NOT NEW.last_read_at
BEGIN
    UPDATE chapter_user
    SET last_modified_at = CASE
        WHEN NEW.last_modified_at <> 0 AND NEW.last_modified_at IS NOT OLD.last_modified_at THEN NEW.last_modified_at
        ELSE MAX(
            CAST(strftime('%s', 'now') AS INTEGER) * 1000 + CAST(substr(strftime('%f', 'now'), 4, 3) AS INTEGER),
            OLD.last_modified_at + 1,
            COALESCE((
                SELECT MAX(sc.modified_at) + 1 FROM sync_change sc
                WHERE sc.user_id = NEW.user_id AND sc.entity_kind = 'chapter_user'
                  AND sc.entity_key = NEW.chapter_id || ':' || NEW.user_id
            ), 0)
        )
    END
    WHERE chapter_id = NEW.chapter_id AND user_id = NEW.user_id;

    INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
    SELECT
        OLD.user_id,
        'manga',
        CAST(sm.manga_id AS TEXT),
        0,
        sm.last_modified_at,
        sm.identity,
        sm.snapshot
    FROM sync_chapter_state scs
    JOIN sync_manga_state sm ON sm.manga_id = scs.manga_id
    JOIN user u ON u.id = OLD.user_id
    WHERE scs.chapter_id = OLD.chapter_id
      AND (OLD.chapter_id IS NOT NEW.chapter_id OR OLD.user_id IS NOT NEW.user_id);

    INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
    SELECT
        OLD.user_id,
        'chapter',
        CAST(scs.chapter_id AS TEXT),
        0,
        CAST(json_extract(scs.snapshot, '$.lastModifiedAt') AS INTEGER),
        scs.identity,
        scs.snapshot
    FROM sync_chapter_state scs
    JOIN user u ON u.id = OLD.user_id
    WHERE scs.chapter_id = OLD.chapter_id
      AND (OLD.chapter_id IS NOT NEW.chapter_id OR OLD.user_id IS NOT NEW.user_id);

    INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
    SELECT
        OLD.user_id,
        'chapter_user',
        OLD.chapter_id || ':' || OLD.user_id,
        1,
        (SELECT last_modified_at FROM chapter_user WHERE chapter_id = NEW.chapter_id AND user_id = NEW.user_id),
        json_set(scs.identity, '$.userId', OLD.user_id),
        json_object(
            'chapterId', OLD.chapter_id,
            'userId', OLD.user_id,
            'lastPageRead', OLD.last_page_read,
            'isRead', OLD.is_read,
            'isBookmarked', OLD.is_bookmarked,
            'lastReadAt', OLD.last_read_at,
            'lastModifiedAt', OLD.last_modified_at
        )
    FROM sync_chapter_state scs
    JOIN user u ON u.id = OLD.user_id
    WHERE scs.chapter_id = OLD.chapter_id
      AND (OLD.chapter_id IS NOT NEW.chapter_id OR OLD.user_id IS NOT NEW.user_id);

    INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
    SELECT
        NEW.user_id,
        'manga',
        CAST(sm.manga_id AS TEXT),
        0,
        sm.last_modified_at,
        sm.identity,
        sm.snapshot
    FROM sync_chapter_state scs
    JOIN sync_manga_state sm ON sm.manga_id = scs.manga_id
    JOIN user u ON u.id = NEW.user_id
    WHERE scs.chapter_id = NEW.chapter_id;

    INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
    SELECT
        NEW.user_id,
        'chapter',
        CAST(scs.chapter_id AS TEXT),
        0,
        CAST(json_extract(scs.snapshot, '$.lastModifiedAt') AS INTEGER),
        scs.identity,
        scs.snapshot
    FROM sync_chapter_state scs
    JOIN user u ON u.id = NEW.user_id
    WHERE scs.chapter_id = NEW.chapter_id;

    INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
    SELECT
        NEW.user_id,
        'chapter_user',
        NEW.chapter_id || ':' || NEW.user_id,
        0,
        (SELECT last_modified_at FROM chapter_user WHERE chapter_id = NEW.chapter_id AND user_id = NEW.user_id),
        json_set(scs.identity, '$.userId', NEW.user_id),
        json_object(
            'chapterId', NEW.chapter_id,
            'userId', NEW.user_id,
            'lastPageRead', NEW.last_page_read,
            'isRead', NEW.is_read,
            'isBookmarked', NEW.is_bookmarked,
            'lastReadAt', NEW.last_read_at,
            'lastModifiedAt', (SELECT last_modified_at FROM chapter_user WHERE chapter_id = NEW.chapter_id AND user_id = NEW.user_id)
        )
    FROM sync_chapter_state scs
    JOIN user u ON u.id = NEW.user_id
    WHERE scs.chapter_id = NEW.chapter_id;
END;

CREATE TRIGGER chapter_user_sync_delete AFTER DELETE ON chapter_user
WHEN EXISTS (SELECT 1 FROM sync_chapter_state scs WHERE scs.chapter_id = OLD.chapter_id)
  AND EXISTS (SELECT 1 FROM user u WHERE u.id = OLD.user_id)
BEGIN
    INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
    SELECT
        OLD.user_id,
        'manga',
        CAST(sm.manga_id AS TEXT),
        0,
        sm.last_modified_at,
        sm.identity,
        sm.snapshot
    FROM sync_chapter_state scs
    JOIN sync_manga_state sm ON sm.manga_id = scs.manga_id
    WHERE scs.chapter_id = OLD.chapter_id;

    INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
    SELECT
        OLD.user_id,
        'chapter',
        CAST(scs.chapter_id AS TEXT),
        0,
        CAST(json_extract(scs.snapshot, '$.lastModifiedAt') AS INTEGER),
        scs.identity,
        scs.snapshot
    FROM sync_chapter_state scs
    WHERE scs.chapter_id = OLD.chapter_id;

    INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
    SELECT
        OLD.user_id,
        'chapter_user',
        OLD.chapter_id || ':' || OLD.user_id,
        1,
        MAX(
            CAST(strftime('%s', 'now') AS INTEGER) * 1000 + CAST(substr(strftime('%f', 'now'), 4, 3) AS INTEGER),
            OLD.last_modified_at + 1
        ),
        json_set(scs.identity, '$.userId', OLD.user_id),
        json_object(
            'chapterId', OLD.chapter_id,
            'userId', OLD.user_id,
            'lastPageRead', OLD.last_page_read,
            'isRead', OLD.is_read,
            'isBookmarked', OLD.is_bookmarked,
            'lastReadAt', OLD.last_read_at,
            'lastModifiedAt', OLD.last_modified_at
        )
    FROM sync_chapter_state scs
    WHERE scs.chapter_id = OLD.chapter_id;
END;

DROP TRIGGER manga_sync_delete;
CREATE TRIGGER manga_sync_delete BEFORE DELETE ON manga
BEGIN
    INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
    SELECT
        fans.user_id,
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
    JOIN (
        SELECT mu.user_id FROM manga_user mu WHERE mu.manga_id = OLD.id
        UNION
        SELECT cu.user_id FROM chapter_user cu
        JOIN chapter c ON c.id = cu.chapter_id
        WHERE c.manga_id = OLD.id
    ) fans ON 1 = 1
    JOIN user u ON u.id = fans.user_id
    WHERE sm.manga_id = OLD.id
    ORDER BY fans.user_id;

    INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
    SELECT
        fans.user_id,
        'chapter',
        CAST(scs.chapter_id AS TEXT),
        1,
        MAX(
            CAST(strftime('%s', 'now') AS INTEGER) * 1000 + CAST(substr(strftime('%f', 'now'), 4, 3) AS INTEGER),
            CAST(json_extract(scs.snapshot, '$.lastModifiedAt') AS INTEGER) + 1
        ),
        scs.identity,
        scs.snapshot
    FROM sync_chapter_state scs
    JOIN (
        SELECT c.id AS chapter_id, mu.user_id
        FROM chapter c
        JOIN manga_user mu ON mu.manga_id = c.manga_id
        WHERE c.manga_id = OLD.id
        UNION
        SELECT cu.chapter_id, cu.user_id FROM chapter_user cu
        JOIN chapter c ON c.id = cu.chapter_id
        WHERE c.manga_id = OLD.id
    ) fans ON fans.chapter_id = scs.chapter_id
    JOIN user u ON u.id = fans.user_id
    WHERE scs.manga_id = OLD.id
    ORDER BY fans.user_id, scs.chapter_id;

    INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
    SELECT
        cu.user_id,
        'chapter_user',
        cu.chapter_id || ':' || cu.user_id,
        1,
        MAX(
            CAST(strftime('%s', 'now') AS INTEGER) * 1000 + CAST(substr(strftime('%f', 'now'), 4, 3) AS INTEGER),
            cu.last_modified_at + 1
        ),
        json_set(scs.identity, '$.userId', cu.user_id),
        json_object(
            'chapterId', cu.chapter_id,
            'userId', cu.user_id,
            'lastPageRead', cu.last_page_read,
            'isRead', cu.is_read,
            'isBookmarked', cu.is_bookmarked,
            'lastReadAt', cu.last_read_at,
            'lastModifiedAt', cu.last_modified_at
        )
    FROM chapter_user cu
    JOIN chapter c ON c.id = cu.chapter_id
    JOIN sync_chapter_state scs ON scs.chapter_id = cu.chapter_id
    JOIN user u ON u.id = cu.user_id
    WHERE c.manga_id = OLD.id
    ORDER BY cu.user_id, cu.chapter_id;

    DELETE FROM sync_chapter_state WHERE manga_id = OLD.id;
END;

DROP TRIGGER source_sync_delete;
CREATE TRIGGER source_sync_delete BEFORE DELETE ON source
BEGIN
    INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
    SELECT
        fans.user_id,
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
    JOIN (
        SELECT mu.manga_id, mu.user_id FROM manga_user mu
        JOIN manga m ON m.id = mu.manga_id
        WHERE m.source_id = OLD.id
        UNION
        SELECT c.manga_id, cu.user_id FROM chapter_user cu
        JOIN chapter c ON c.id = cu.chapter_id
        JOIN manga m ON m.id = c.manga_id
        WHERE m.source_id = OLD.id
    ) fans ON fans.manga_id = sm.manga_id
    JOIN user u ON u.id = fans.user_id
    WHERE sm.source_id = OLD.id
    ORDER BY fans.user_id, sm.manga_id;

    INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
    SELECT
        fans.user_id,
        'chapter',
        CAST(scs.chapter_id AS TEXT),
        1,
        MAX(
            CAST(strftime('%s', 'now') AS INTEGER) * 1000 + CAST(substr(strftime('%f', 'now'), 4, 3) AS INTEGER),
            CAST(json_extract(scs.snapshot, '$.lastModifiedAt') AS INTEGER) + 1
        ),
        scs.identity,
        scs.snapshot
    FROM sync_chapter_state scs
    JOIN (
        SELECT c.id AS chapter_id, mu.user_id
        FROM chapter c
        JOIN manga m ON m.id = c.manga_id
        JOIN manga_user mu ON mu.manga_id = c.manga_id
        WHERE m.source_id = OLD.id
        UNION
        SELECT cu.chapter_id, cu.user_id FROM chapter_user cu
        JOIN chapter c ON c.id = cu.chapter_id
        JOIN manga m ON m.id = c.manga_id
        WHERE m.source_id = OLD.id
    ) fans ON fans.chapter_id = scs.chapter_id
    JOIN user u ON u.id = fans.user_id
    WHERE scs.source_id = OLD.id
    ORDER BY fans.user_id, scs.manga_id, scs.chapter_id;

    INSERT INTO sync_change (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot)
    SELECT
        cu.user_id,
        'chapter_user',
        cu.chapter_id || ':' || cu.user_id,
        1,
        MAX(
            CAST(strftime('%s', 'now') AS INTEGER) * 1000 + CAST(substr(strftime('%f', 'now'), 4, 3) AS INTEGER),
            cu.last_modified_at + 1
        ),
        json_set(scs.identity, '$.userId', cu.user_id),
        json_object(
            'chapterId', cu.chapter_id,
            'userId', cu.user_id,
            'lastPageRead', cu.last_page_read,
            'isRead', cu.is_read,
            'isBookmarked', cu.is_bookmarked,
            'lastReadAt', cu.last_read_at,
            'lastModifiedAt', cu.last_modified_at
        )
    FROM chapter_user cu
    JOIN sync_chapter_state scs ON scs.chapter_id = cu.chapter_id
    JOIN user u ON u.id = cu.user_id
    WHERE scs.source_id = OLD.id
    ORDER BY cu.user_id, scs.manga_id, cu.chapter_id;

    DELETE FROM sync_chapter_state WHERE source_id = OLD.id;
END;
