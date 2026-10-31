-- Field locks follow an entry through source refreshes, backups and sync, so
-- they live on the row the rest of its metadata lives on.
ALTER TABLE manga ADD COLUMN metadata_locks TEXT NOT NULL DEFAULT '{}';

-- The journal snapshot is what peers apply, so the lock map travels with it.
-- Stores keep the view current in code; 0034 removed the triggers.
DROP VIEW IF EXISTS sync_manga_state;
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
        'metadataLocks', json(m.metadata_locks),
        'lastModifiedAt', m.last_modified_at
    ) AS snapshot
FROM manga m
JOIN source s ON s.id = m.source_id;
