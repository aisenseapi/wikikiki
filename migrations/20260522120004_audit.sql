-- Audit and access log (DESIGN.md §3.1, §14).
-- admin_audit: lookup-table / config / credentials / capability changes.
-- access_log : every login attempt, denied read/write, token use.

CREATE TABLE admin_audit (
    id            INTEGER PRIMARY KEY,
    actor_id      INTEGER NOT NULL REFERENCES actors(id),
    target        TEXT NOT NULL,        -- e.g. 'actors:42', 'credentials:7', 'config:server.bind'
    operation     TEXT NOT NULL,        -- 'insert' | 'update' | 'delete' | 'issue_token' | 'revoke_token' | 'reload'
    before_value  TEXT,                 -- JSON or markdown (NEVER plaintext secrets)
    after_value   TEXT,                 -- JSON or markdown (NEVER plaintext secrets)
    note          TEXT,                 -- markdown
    created_at    INTEGER NOT NULL
);

CREATE INDEX idx_admin_audit_actor ON admin_audit(actor_id, created_at);

CREATE TABLE access_log (
    id                INTEGER PRIMARY KEY,
    actor_id          INTEGER REFERENCES actors(id),  -- null on pre-resolution failure
    attempted_handle  TEXT,                            -- for failed logins
    surface           TEXT NOT NULL,                   -- 'ui' | 'api' | 'mcp'
    outcome           TEXT NOT NULL,                   -- 'login_success' | 'login_failure' | 'denied_read' | 'denied_write' | 'token_used'
    target            TEXT,
    remote_addr       TEXT,                            -- hashed/pseudonymized per privacy policy
    created_at        INTEGER NOT NULL
);

CREATE INDEX idx_access_log_actor_time ON access_log(actor_id, created_at);
CREATE INDEX idx_access_log_outcome ON access_log(outcome, created_at);

-- Tombstones for soft-deleted content (Stage 4 hardening; table shipped now
-- so the safety-net surface is in place from v1).
CREATE TABLE tombstones (
    id             INTEGER PRIMARY KEY,
    source_kind    TEXT NOT NULL,
    source_id      INTEGER NOT NULL,
    snapshot       TEXT NOT NULL,        -- full markdown content at deletion
    deleted_by     INTEGER NOT NULL REFERENCES actors(id),
    deleted_at     INTEGER NOT NULL,
    expires_at     INTEGER NOT NULL,
    recovered_at   INTEGER,
    recovered_by   INTEGER REFERENCES actors(id)
);
