-- Tri-state EPUB analysis: NULL for non-EPUB or unknown, 0 for non-divina, 1 for divina.
ALTER TABLE chapter ADD COLUMN epub_divina_compatible INTEGER;
