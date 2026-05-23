-- Lookup tables (DESIGN.md §4, §13). No CHECK constraints on enumerated values;
-- everything that could grow new values is a row, not a literal in code.

CREATE TABLE actor_types (
    code         TEXT PRIMARY KEY,
    label        TEXT NOT NULL,
    description  TEXT,
    is_active    INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE layers (
    code           TEXT PRIMARY KEY,
    label          TEXT NOT NULL,
    description    TEXT,
    git_versioned  INTEGER NOT NULL DEFAULT 0,
    sort_order     INTEGER NOT NULL,
    is_active      INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE thread_states (
    code         TEXT PRIMARY KEY,
    label        TEXT NOT NULL,
    description  TEXT,
    is_terminal  INTEGER NOT NULL DEFAULT 0,
    is_active    INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE state_transitions (
    from_state  TEXT NOT NULL REFERENCES thread_states(code),
    to_state    TEXT NOT NULL REFERENCES thread_states(code),
    description TEXT,
    PRIMARY KEY (from_state, to_state)
);

CREATE TABLE distillation_actions (
    code        TEXT PRIMARY KEY,
    label       TEXT NOT NULL,
    description TEXT,
    is_active   INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE event_kinds (
    code        TEXT PRIMARY KEY,
    label       TEXT NOT NULL,
    description TEXT,
    is_active   INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE subscription_target_kinds (
    code      TEXT PRIMARY KEY,
    label     TEXT NOT NULL,
    is_active INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE thread_visibilities (
    code        TEXT PRIMARY KEY,
    label       TEXT NOT NULL,
    description TEXT,
    is_active   INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE source_kinds (
    code        TEXT PRIMARY KEY,
    label       TEXT NOT NULL,
    description TEXT,
    is_active   INTEGER NOT NULL DEFAULT 1
);

-- ===== Seed initial vocabulary. =====

INSERT INTO actor_types(code, label, description) VALUES
  ('human',    'Human',    'A person, acting through the web UI.'),
  ('agent',    'Agent',    'An AI agent, acting through the agent API.'),
  ('system',   'System',   'Internal processes — distillation, cleanup, scheduled jobs.'),
  ('external', 'External', 'Other systems, webhooks, federated wikis.');

INSERT INTO layers(code, label, description, git_versioned, sort_order) VALUES
  ('working',  'Working memory',  'Live state. SQLite only, not versioned in git.', 0, 1),
  ('episodic', 'Episodic memory', 'What happened. Markdown files in git.',           1, 2),
  ('semantic', 'Semantic memory', 'Established knowledge. Markdown files in git.',   1, 3);

INSERT INTO thread_states(code, label, is_terminal) VALUES
  ('opened',     'Opened',     0),
  ('active',     'Active',     0),
  ('paused',     'Paused',     0),
  ('distilling', 'Distilling', 0),
  ('closed',     'Closed',     1);

INSERT INTO state_transitions(from_state, to_state) VALUES
  ('opened',     'active'),
  ('active',     'paused'),
  ('paused',     'active'),
  ('active',     'distilling'),
  ('distilling', 'active'),
  ('distilling', 'closed'),
  ('active',     'closed');

INSERT INTO distillation_actions(code, label, description) VALUES
  ('merge',      'Merge',      'Merge into the target main page.'),
  ('append',     'Append',     'Add as a sub-page.'),
  ('contradict', 'Contradict', 'Replace main with new position; preserve previous as dated sub-page.'),
  ('refactor',   'Refactor',   'Consolidate main and sub-pages into a cleaner shape.'),
  ('discard',    'Discard',    'Decline to lift — recorded for audit but not promoted.');

INSERT INTO event_kinds(code, label, description) VALUES
  ('message',      'Message',      'Free-form thread message.'),
  ('state_change', 'State change', 'Thread state machine transition.'),
  ('distillation', 'Distillation', 'A distillation acted on this thread.'),
  ('note',         'Note',         'Annotation or aside.'),
  ('loop_alert',   'Loop alert',   'Multi-actor edit churn detected.');

INSERT INTO subscription_target_kinds(code, label) VALUES
  ('thread', 'Thread'),
  ('page',   'Page'),
  ('global', 'Global');

INSERT INTO thread_visibilities(code, label, description) VALUES
  ('public',       'Public',       'Any actor can read; any actor can write.'),
  ('participants', 'Participants', 'Only listed participants can read or write.'),
  ('actor_only',   'Actor only',   'Only the opening actor can read or write.');

INSERT INTO source_kinds(code, label, description) VALUES
  ('thread',              'Thread',              'A thread.'),
  ('page',                'Page',                'A wiki page.'),
  ('wm_edits',            'Working-memory edits','A range of working-memory edits.'),
  ('pruned_l1',           'Pruned L1',           'An L1 entry being discarded by the pruner.'),
  ('conflict_resolution', 'Conflict resolution', 'A conflict-branch resolution.'),
  ('migration',           'Migration',           'A migration-time data move.');
