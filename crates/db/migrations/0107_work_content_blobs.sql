-- Content-addressable dedup for work-document versions. Version bytes are stored
-- once at <data_dir>/files/work/blobs/<hash>.<ext>; many versions (a revert, the
-- same content across documents) can reference the same blob. This table is the
-- registry of stored blobs — it enables size accounting and future GC of blobs no
-- version references anymore.
CREATE TABLE IF NOT EXISTS work_content_blobs (
    hash        TEXT PRIMARY KEY,                  -- SHA-256 of the content
    ext         TEXT NOT NULL,                     -- extension (drives content-type on serve)
    size_bytes  INTEGER NOT NULL,
    created_at  INTEGER NOT NULL DEFAULT (unixepoch())
);
