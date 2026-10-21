-- 记录每键上次播种的默认值：源升级时仅刷新「值仍等于旧默认」的行，用户改过的不动
ALTER TABLE source_setting ADD COLUMN seeded_value TEXT;
