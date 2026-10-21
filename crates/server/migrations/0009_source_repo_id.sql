-- 记录源从哪个仓库安装（NULL = 目录扫描侧载）；
-- 仓库安装的源若不在任何仓库 index 里，source_json 据此报 missingFromRepos
ALTER TABLE source ADD COLUMN repo_id INTEGER;
