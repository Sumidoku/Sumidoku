-- Persists the source-side reading direction and chapter thumbnails; viewer values match the aidoku Viewer enum (starting at 0=Unknown)
ALTER TABLE manga ADD COLUMN viewer INTEGER NOT NULL DEFAULT 0;
ALTER TABLE chapter ADD COLUMN thumbnail TEXT;
