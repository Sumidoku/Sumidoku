-- Stores the serialized SourceFeatures verbatim at registration; the DTO exposes capability bits like hasHome from it;
-- NULL = old rows never stamped, consumers treat it as unknown (entry points are not hidden because of it)
ALTER TABLE source ADD COLUMN features TEXT;
