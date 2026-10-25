-- Local EPUB pages are derived data; caches written by older divina parsers must be rebuilt.
DELETE FROM page
WHERE chapter_id IN (
    SELECT chapter.id
    FROM chapter
    JOIN manga ON manga.id = chapter.manga_id
    JOIN source ON source.id = manga.source_id
    WHERE source.kind = 'local' AND lower(chapter.key) LIKE '%.epub'
);

UPDATE chapter
SET page_count = 0
WHERE id IN (
    SELECT chapter.id
    FROM chapter
    JOIN manga ON manga.id = chapter.manga_id
    JOIN source ON source.id = manga.source_id
    WHERE source.kind = 'local' AND lower(chapter.key) LIKE '%.epub'
);
