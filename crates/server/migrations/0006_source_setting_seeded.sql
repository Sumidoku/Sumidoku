-- Records the last seeded default value per key: on source upgrade only rows whose value still equals the old default are refreshed; user-modified rows are left alone
ALTER TABLE source_setting ADD COLUMN seeded_value TEXT;
