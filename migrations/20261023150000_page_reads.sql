-- One row per authenticated page read, with the revision that was served.
-- This is the cheapest dependency evidence the server can collect: it
-- records which actor saw which version, without asking any client to
-- declare what it relied on. A read is a candidate signal, not proof that
-- the page influenced later work. Anonymous reads have no actor to record.
CREATE TABLE page_reads (
    id          INTEGER PRIMARY KEY,
    page_id     INTEGER NOT NULL REFERENCES pages(id),
    actor_id    INTEGER NOT NULL REFERENCES actors(id),
    revision    TEXT,                       -- NULL for a legacy row without one
    surface     TEXT NOT NULL,              -- 'ui' or 'api'
    created_at  INTEGER NOT NULL
);

CREATE INDEX idx_page_reads_page_actor_time ON page_reads(page_id, actor_id, created_at);
CREATE INDEX idx_page_reads_actor_time ON page_reads(actor_id, created_at);
