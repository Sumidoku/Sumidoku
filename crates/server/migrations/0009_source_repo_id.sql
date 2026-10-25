-- Records which repo a source was installed from (NULL = sideloaded via directory scan);
-- if a repo-installed source is not in any repo index, source_json uses this to report missingFromRepos
ALTER TABLE source ADD COLUMN repo_id INTEGER;
