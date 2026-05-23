-- Actors and credentials (DESIGN.md §13, §14).
-- The actors table is the truth about identity. Profile *pages* (narrative
-- markdown) live elsewhere and are editable by humans; this table is not.

CREATE TABLE actors (
    id            INTEGER PRIMARY KEY,
    type          TEXT NOT NULL REFERENCES actor_types(code),
    name          TEXT NOT NULL,
    handle        TEXT NOT NULL UNIQUE,
    metadata      TEXT,                       -- JSON or markdown frontmatter
    is_admin      INTEGER NOT NULL DEFAULT 0, -- elevated capability flag
    is_active     INTEGER NOT NULL DEFAULT 1,
    created_at    INTEGER NOT NULL,
    last_seen_at  INTEGER
);

CREATE INDEX idx_actors_type ON actors(type, is_active);

-- One actor may have multiple credentials (password + multiple tokens, etc.).
-- secret_hash stores argon2; plaintext is only shown once at issuance.
CREATE TABLE credentials (
    id           INTEGER PRIMARY KEY,
    actor_id     INTEGER NOT NULL REFERENCES actors(id),
    kind         TEXT NOT NULL,        -- 'password' | 'token' | 'oauth' | 'sso' | 'system_token'
    -- lookup_key is the un-hashed prefix of a token (16 hex chars). Tokens
    -- ship as `wk_<lookup_key>_<secret>`; on verification we look up by
    -- lookup_key for O(1) row selection, then argon2-verify the secret.
    -- Null for credentials that aren't keyed (password rows).
    lookup_key   TEXT UNIQUE,
    secret_hash  TEXT NOT NULL,
    label        TEXT,
    last_used_at INTEGER,
    expires_at   INTEGER,              -- null = no expiry
    revoked_at   INTEGER,              -- non-null = revoked
    created_at   INTEGER NOT NULL
);

CREATE INDEX idx_credentials_actor ON credentials(actor_id, kind);
CREATE INDEX idx_credentials_kind_live
    ON credentials(kind)
    WHERE revoked_at IS NULL;

-- Web sessions. Cookie carries the (random) `id`; we look up the actor.
-- Kept separate from `credentials` because sessions expire fast and churn
-- a lot, while credentials are long-lived.
CREATE TABLE sessions (
    id          TEXT PRIMARY KEY,         -- random token (server side)
    actor_id    INTEGER NOT NULL REFERENCES actors(id),
    created_at  INTEGER NOT NULL,
    expires_at  INTEGER NOT NULL,
    revoked_at  INTEGER
);

CREATE INDEX idx_sessions_actor ON sessions(actor_id, expires_at);
