DELETE FROM sync_change WHERE origin_user_id != 1;
DELETE FROM sync_alias WHERE origin_user_id != 1;
DELETE FROM sync_origin_state WHERE origin_user_id != 1;
DELETE FROM setting WHERE key LIKE 'user.%' AND key NOT LIKE 'user.1.%';
UPDATE user SET role = 'admin' WHERE id = 1;
DELETE FROM user WHERE id != 1;
INSERT OR IGNORE INTO setting (key, value) VALUES ('sync.single_owner_rebaseline', 'true');
