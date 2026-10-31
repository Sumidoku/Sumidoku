-- Stores now write journal entries in the transaction that changes the row, and sync_chapter_state
-- is kept current by store::chapter, so the triggers that did both are gone.
DROP TRIGGER IF EXISTS category_sync_insert;
DROP TRIGGER IF EXISTS category_sync_update;
DROP TRIGGER IF EXISTS category_sync_delete;
DROP TRIGGER IF EXISTS category_manga_sync_insert;
DROP TRIGGER IF EXISTS category_manga_sync_update;
DROP TRIGGER IF EXISTS category_manga_sync_delete;
DROP TRIGGER IF EXISTS chapter_sync_insert;
DROP TRIGGER IF EXISTS chapter_sync_update;
DROP TRIGGER IF EXISTS chapter_sync_delete;
DROP TRIGGER IF EXISTS chapter_sync_state_version_update;
DROP TRIGGER IF EXISTS chapter_user_sync_insert;
DROP TRIGGER IF EXISTS chapter_user_sync_update;
DROP TRIGGER IF EXISTS chapter_user_sync_delete;
DROP TRIGGER IF EXISTS manga_sync_insert;
DROP TRIGGER IF EXISTS manga_sync_update;
DROP TRIGGER IF EXISTS manga_sync_delete;
DROP TRIGGER IF EXISTS manga_sync_chapter_state_update;
DROP TRIGGER IF EXISTS manga_user_sync_insert;
DROP TRIGGER IF EXISTS manga_user_sync_update;
DROP TRIGGER IF EXISTS manga_user_sync_delete;
DROP TRIGGER IF EXISTS source_sync_delete;
DROP TRIGGER IF EXISTS source_sync_chapter_state_update;
DROP TRIGGER IF EXISTS sync_change_origin_insert;
