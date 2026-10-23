-- A resource revision changes on every accepted write, including layer-only
-- changes. Preserve existing client revisions until their next successful edit.
ALTER TABLE pages ADD COLUMN revision TEXT;
UPDATE pages SET revision = content_hash;
ALTER TABLE pages ADD COLUMN projection_version INTEGER NOT NULL DEFAULT 0;

-- Fail migration rather than silently choosing between existing aliases.
-- Wiki paths are ASCII, so SQLite lower() is the portable comparison key.
CREATE UNIQUE INDEX idx_pages_portable_path ON pages(lower(path));

ALTER TABLE page_edits ADD COLUMN operation_id TEXT;
ALTER TABLE page_edits ADD COLUMN layer TEXT REFERENCES layers(code);
CREATE UNIQUE INDEX idx_page_edits_operation
    ON page_edits(operation_id) WHERE operation_id IS NOT NULL;

-- The intent is durable before Git is touched. It is removed in the same
-- transaction that publishes metadata, attribution and the search projection.
-- Content is retained here so a failed materialization can be retried after
-- restart without relying on mutable working-tree files.
CREATE TABLE pending_page_writes (
    operation_id         TEXT PRIMARY KEY,
    page_path            TEXT NOT NULL UNIQUE,
    repository_path      TEXT NOT NULL,
    layer                TEXT NOT NULL REFERENCES layers(code),
    content              TEXT NOT NULL,
    content_hash         TEXT NOT NULL,
    actor_id             INTEGER NOT NULL REFERENCES actors(id),
    actor_type           TEXT NOT NULL,
    actor_handle         TEXT NOT NULL,
    author_signature     TEXT NOT NULL,
    summary              TEXT,
    created_at           INTEGER NOT NULL,
    base_commit          TEXT NOT NULL,
    previous_content_hash TEXT
);
