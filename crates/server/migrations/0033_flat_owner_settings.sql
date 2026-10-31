-- The server has one owner, so owner settings lose their user.1. namespace. A namespaced row wins
-- over a bare one because reads only ever saw the namespaced value.
INSERT OR REPLACE INTO setting (key, value)
SELECT substr(key, length('user.1.') + 1), value FROM setting WHERE key LIKE 'user.1.%';

DELETE FROM setting WHERE key LIKE 'user.%';
