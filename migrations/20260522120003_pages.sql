-- Page metadata. Markdown content lives on disk under paths.content_root.
-- pages_fts indexes everything for FTS5 search.

CREATE TABLE pages (
    id            INTEGER PRIMARY KEY,
    path          TEXT NOT NULL UNIQUE,
    layer         TEXT NOT NULL REFERENCES layers(code),
    parent_id     INTEGER REFERENCES pages(id),
    title         TEXT NOT NULL,
    maintainer    INTEGER REFERENCES actors(id),
    content_hash  TEXT,                       -- sha256 of file content
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL,
    last_edit_at  INTEGER,
    last_edit_by  INTEGER REFERENCES actors(id)
);

CREATE INDEX idx_pages_layer ON pages(layer);
CREATE INDEX idx_pages_parent ON pages(parent_id);

CREATE TABLE page_edits (
    id          INTEGER PRIMARY KEY,
    page_id     INTEGER NOT NULL REFERENCES pages(id),
    actor_id    INTEGER NOT NULL REFERENCES actors(id),
    git_commit  TEXT,
    summary     TEXT,                  -- markdown
    created_at  INTEGER NOT NULL
);

CREATE INDEX idx_page_edits_page ON page_edits(page_id, created_at DESC);

-- FTS5 index over title + content + UNINDEXED path. We don't use
-- contentless mode (`content=''`) because that breaks snippet()/highlight()
-- — Stage 1's UI needs both. The duplication of the markdown text into
-- the FTS index is acceptable at this scale; we revisit if the index size
-- becomes painful.
CREATE VIRTUAL TABLE pages_fts USING fts5(
    title,
    content,
    path UNINDEXED
);
