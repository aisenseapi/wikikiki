# Wikikiki — Design Document

> A wiki that is the live memory of a system of actors — human, agent, and machine — observable and editable in real time.

**Stack:** Rust · SQLite (with FTS5) · Markdown · Git · Web UI
**Status:** Design draft — to be refined as the project evolves.

---

## 1. Vision

Wikikiki is not a wiki *with* AI features. It is a wiki that **is the working memory of a system of actors**, with a web interface that makes that memory observable and editable in real time.

### Actors

Everything that reads or writes is an **actor**. Actors are typed; the type determines defaults (UI affordances, attribution style, rate limits), but the underlying capability model is the same for all.

Initial actor types:
- `human` — a person, acting through the web UI.
- `agent` — an AI agent, acting through the agent API.
- `system` — internal processes (distillation sweeps, cleanup, scheduled jobs).
- *(future)* `external` — other systems, webhooks, federated wikis.

Every write carries an `actor_id` and `actor_type`. Nothing in the data model hardcodes "human" vs "agent" — they are values, not branches in the schema.

### Actor roles (what they typically do)

Roles describe behavior, not identity. An actor may take on multiple roles over time.

- **Observers** watch live state (typically `human` via web UI, but any actor with read access can observe).
- **Contributors** read, write, and refine — the bread-and-butter loop (typically `agent`, but `human` contributes too).
- **Correctors** edit, add, or delete to fix mistakes or add context (typically `human`, but agents can correct each other when warranted).
- **Distillers** evaluate material and lift it between layers (typically `system` or specialized `agent`, occasionally `human`).

No approval gates. No queues. The default posture is trust + observation; correction happens in flow. The corresponding safety net is audit + rollback + transparency, not gating (see §3).

The project lives at: `github.com/<owner>/wikikiki` (repo prepared).

---

## 2. What Is Fixed, What Is Configurable

A real risk in this design is that "no hardcoding" tips over into "everything is configurable, nothing is committed to." That makes a framework, not a product. So we draw a clear line.

### The core (fixed — not configurable, not negotiable)

These are the ideas that *make Wikikiki Wikikiki*. Removing or weakening any of them produces a different product.

1. **A memory gradient exists.** There is rough/live material, there is what-happened material, and there is established-knowledge material. The names, counts, and properties of layers are data; the *existence of the gradient* is the product.
2. **Distillation is a first-class operation.** Material flows up the gradient via explicit, attributed acts of distillation. This is the engine of the system.
3. **All readers and writers are actors of equal modeling weight.** `human`, `agent`, `system`, `external` are values, not branches in the schema.
4. **Markdown is the universal content format.** Plain text is the substrate; the web UI is a renderer over it, not a separate document system.
5. **No approval gates by default.** Actors act directly; corrections happen in flow — balanced by a strong safety net (§3).
6. **Everything is observable and attributed.** Every read- or write-capable surface emits events; every change carries an actor and timestamp.
7. **Additive by default.** Lifting material to a higher layer never destroys the source. Information loss is the failure we fear most.

These principles do not live in config. They live in the architecture and in this document.

### The shell (configurable — operator-tunable)

Everything that is an *implementation detail* of the core principles lives in config or lookup tables:

- Names, counts, and properties of layers (still must satisfy the gradient).
- Actor type vocabulary (still must conform to the equal-modeling rule).
- State machine states and transitions.
- Distillation action vocabulary (`merge`, `append`, etc.).
- Thresholds (retention, distillation triggers, rate limits).
- Paths, ports, transports, auth modes, renderer choices.

The test: *does changing this value break a core principle above?* If yes, it's part of the core. If no, it's part of the shell.

---

## 3. The Safety Net (What Replaces Approval Gates)

Saying "no gates" without a safety net is naïve. The safety net has four parts — all first-class system features, not optional add-ons.

### 3.1 Full audit trail

Every write — page edit, working-memory update, thread event, distillation, state transition, branch creation, soft delete — is recorded with `actor_id`, `actor_type`, timestamp, and (where applicable) git commit hash. **Nothing happens in Wikikiki that is not attributable to an actor.**

- L2/L3: git provides this natively (commit author = actor).
- L1: `working_memory_edits` (every change to working memory), `thread_events` (the narrative), and `page_edits` (page-level changes) together capture every L1 mutation. The split between current state (`working_memory`) and edit history (`working_memory_edits`) is enforced in the schema (§5, §13) — upserts cannot silently discard history.
- Admin actions (lookup-table edits, config reloads): a dedicated `admin_audit` table.

### 3.2 Reversibility (rollback)

Anything that was committed can be reverted. Anything soft-deleted has a tombstone for a configurable grace period (`safety.tombstone_retention`) before hard deletion. Conflict-resolution branches are preserved indefinitely so a "wrong merge" can be undone by checking out the alternative branch.

**Principle:** deletion is never irreversible inside the configured retention window.

### 3.3 Visible conflicts

Conflicts are never silent. When two actors disagree — concurrent edits, contradictory distillations, a `system` action a `human` disputes — the conflict materializes as a **visible review thread in L1**, addressable by any actor in the same memory model. Not a queue, not a hidden notification — an object in the substrate everyone already watches. See §14 for the concurrent-edit mechanism; the same pattern applies to distillation disputes.

### 3.4 Clear accountability per actor

Every actor has a profile *page* (auto-generated, editable as any other wiki page) showing what they have written, what they subscribe to, and what they are currently working on. The page is a markdown wiki page like any other; the **actor's identity** (type, handle, capabilities) lives in the `actors` table and is *not* editable through the page. The page is narrative about the actor; the table row is the truth about the actor.

This separation matters: a human can annotate an agent's profile page ("this agent tends to over-summarize") without being able to change the agent's type or handle. Observability without privilege escalation.

**Rate limits per actor type** are configurable (`limits.<actor_type>.l1_writes_per_minute`, etc.) — defaults ship that prevent runaway loops without slowing normal work.

**Duplicate suppression** complements rate limits: identical L1 writes from the same actor within `distillation.dedup_window` (default 1 minute) are collapsed into a single entry with an incremented count. A stuck-in-a-loop agent then appears as one entry that keeps re-firing, not thousands of identical rows polluting the substrate. Observers can see the loop pattern clearly.

### 3.5 Loop detection across actors

Rate limits catch a single agent looping; duplicate suppression catches identical content. Neither catches the more subtle pattern of *two or more agents correcting each other* — A edits, B re-edits, A re-edits, indefinitely. This is a real risk in any system where multiple agents can write to the same surface.

The safety mechanism is **edit churn detection**: when a page accumulates more than a threshold number of edits by different non-human actors within a configurable window, with no `human` actor in between, the system opens a review thread (`kind='loop_alert'`) flagging the pattern.

- The thresholds (`safety.loop_detection.max_edits`, `safety.loop_detection.window`) are configurable so deployments can tune sensitivity without code changes.
- The alert is non-blocking — edits still proceed. The thread just makes the pattern visible to observers.
- Specific heuristics (which actors count, how to weight types) are an implementation choice and may evolve; the *principle* is that multi-actor edit storms must be visible.

---

## 4. No Hardcoding — Configuration & Extensibility

A core principle: **specifics live in configuration, not in code or schemas.** Things that change between deployments, that an operator might want to tune, or that might grow new values over time, all live in one of three places (in order of preference):

1. **Database** — for values that change at runtime or need to be edited via the UI (actor types, state machine definitions, layer definitions, distillation actions).
2. **Config file (TOML)** — for values set at deploy time (paths, thresholds, transport choices, feature flags).
3. **Environment variables** — for secrets and per-environment overrides (tokens, hostnames, ports, debug flags).

What this rules out: `CHECK (... IN ('a','b','c'))` constraints, magic numbers in code, hardcoded paths, hardcoded state names, hardcoded action types. If you can spell out a value as a literal in source code, ask whether it should be data instead.

### Config file (TOML, sketch)

```toml
# wikikiki.toml — operator-facing configuration

[paths]
content_root      = "./content"           # where markdown files live
db_path           = "./wikikiki.db"
git_repo          = "./content"           # may equal content_root

[server]
bind              = "127.0.0.1:8090"
public_url        = "http://localhost:8090"

[memory.working]
prune_after_days  = 30                    # L1 entries unlifted for this long are removed
prune_interval    = "1h"                  # how often the pruner runs

[memory.episodic]
commit_strategy   = "on_state_change"     # or "batched", "manual"
commit_batch_size = 10

[memory.semantic]
default_layout    = "main_with_subpages"  # how new topics are laid out

[distillation]
sweep_interval        = "15m"
inactivity_threshold  = "2h"              # threads idle this long get evaluated
length_threshold      = 50                # events; threshold for size-based trigger
l3_maturation         = "7d"              # proposed L3 edits auto-mature to stable after this
dedup_window          = "1m"              # collapse identical L1 writes within this window

[realtime]
transport         = "sse"                 # or "websocket", "both"
heartbeat         = "30s"

[mcp]
enabled           = true
bind              = "127.0.0.1:8081"      # MCP server endpoint (separate from web)
transport         = "sse"                 # or "stdio" for local agents

[ui]
markdown_renderer = "comrak"
allow_anonymous_read = true
require_auth_for_write = true

[auth]
session_lifetime  = "30d"                 # cookie lifetime for human sessions
token_default_lifetime = "1y"             # default expiry on issued tokens; null = no expiry
password_hash     = "argon2id"
oauth_providers   = []                    # list of configured OAuth providers (empty by default)
log_remote_addr   = "hashed"              # 'plain','hashed','none' — for access_log entries

[git]
implementation    = "gix"                 # or "git2"
author_template   = "{actor_type}:{actor_handle} <{actor_handle}@wikikiki.local>"

[layers]
# Layer definitions live in DB (see schema) so they can be edited live.
# This is just an override for initial seeding.

[safety]
tombstone_retention   = "30d"             # soft-deleted content recoverable for this long
preserve_conflict_branches = true         # keep merge-conflict branches indefinitely
admin_audit           = true              # log all lookup-table/config changes
system_distillation_retention = "1y"      # how long to keep system 'discard' distillations
[safety.loop_detection]
max_edits             = 6                 # threshold of non-human edits in window before alert
window                = "10m"             # the rolling window

[limits.agent]
l1_writes_per_minute  = 600
l3_writes_per_minute  = 30
[limits.external]
l1_writes_per_minute  = 120
l3_writes_per_minute  = 10
# (human and system have no defaults — system is trusted, human is rate-limited at UI level)

[conflict]
stale_timeout         = "3d"              # review threads without action fall back after this

[git.conflict]
branch_retention      = "30d"             # conflict branches older than this with closed thread are gc'd
```

Environment variables override config file values; config file values override built-in defaults. Every config value has a sensible default so the system runs with an empty config file.

### Lookup tables (DB-driven extensibility)

Anything that could grow new values lives in a lookup table rather than a CHECK constraint:

- `actor_types` — `human`, `agent`, `system`, `external`, *and anything you add later*.
- `thread_states` — full state machine definition (states + allowed transitions).
- `layers` — memory layers (initially 3, but the count and properties are not baked in).
- `distillation_actions` — `merge`, `append`, `contradict`, *or new ones*.
- `event_kinds` — kinds of thread events.

Initial seed data is provided in migrations; everything is editable at runtime via admin endpoints or directly in the database.

---

## 5. The Three-Layer Memory Model

Memory is organized into three layers with different properties. This is the most important structural idea in the system.

### Layer 1 — Working Memory (live state)

- **Storage:** SQLite only. *Not* versioned in git.
- **Lifetime:** Seconds to days. Aggressively pruned.
- **Content:** The current state of active threads — open questions, current goals, hypotheses in progress, intermediate computations, conversation buffers. Stored as markdown text (see §10).
- **Access:** actors read/write many times per second. Other actors observe live via the configured real-time transport (SSE/WebSocket).
- **Why not git:** Committing every micro-update would drown the history in noise and make git useless as a record.
- **State vs. history (important).** L1 separates two concerns that are easy to conflate:
  - `working_memory` — *the current value of each key.* One row per `(thread, key)`. Upserted in place.
  - `working_memory_edits` — *the append-only history of every change to that value.* Written in the same transaction as every upsert.
  - `thread_events` — *the stream of what happened in the thread.* State changes, distillation events, messages — the narrative, not the state.
  
  The schema enforces this split (§13). Conflating them — letting `working_memory` carry its own history via versioned rows — would either bloat the live state or silently erase the audit trail promised in §3.

### Layer 2 — Episodic Memory (what happened)

- **Storage:** Markdown files, versioned in git. Raw data may stay in SQLite with pointers.
- **Lifetime:** Long. Periodically reviewed but rarely deleted.
- **Content:** Distilled summaries of threads — what was worked on, what was decided, what was learned. The "logbook."
- **Granularity:** Commits happen at meaningful state transitions, not per message. A commit message reads like an entry: *"Thread #142 resolved: agreed on additive merge strategy."*

### Layer 3 — Semantic Memory (established knowledge)

- **Storage:** Markdown files, versioned in git. The classic wiki layer.
- **Lifetime:** Indefinite. Edited, merged, refined over time.
- **Content:** Reference knowledge actors look up — conventions, facts, definitions, established patterns. Any actor can write here.
- **Structure:** Main page / sub-page pattern (see §7).

### Transitions between layers

Material flows upward through **distillation** (see §8). The flow is **additive by default**: when something is lifted from L1 to L2 or L2 to L3, the source is not destroyed — it remains as raw material linked from the distillate.

Periodic cleanup removes L1 entries older than the configured retention period (`memory.working.prune_after_days`) that were never lifted. L2 and L3 are kept indefinitely; git handles history.

**Pruning is not silent.** Every pruned L1 entry is recorded as a `system`-actor distillation with `action='discard'` and rationale "L1 retention expired without distillation", referencing the entry's content as `source_snapshot`. This means:

- The audit trail (§3.1) is preserved — discarding is always an *attributed act*, never a silent reaper.
- A reviewer can later run "show me all `system` discards from the last week" and see exactly what the system threw away, with content snapshots.
- If pruning was a mistake (something *should* have been lifted), the snapshot is still there until the `system` distillation itself is pruned, with a much longer retention window (`safety.system_distillation_retention`, default 1 year).

This is the operational meaning of "additive by default": even forgetting is a recorded act.

The current three layers are the initial conception, but the **number and nature of layers is data, not code** (see `layers` lookup table). A deployment could add a fourth layer (e.g. "archive") or collapse to two without code changes.

---

## 6. Cross-Actor Edits as First-Class Events

When one actor edits content another actor relies on, the other must notice. This is not specific to humans correcting agents — it applies to any combination (agent correcting agent, system pruning what a human wrote, etc.).

- Every page tracks `last_edit_at` plus a per-actor-type breakdown (e.g. `last_edit_by_type` as a JSON column or join table) so consumers can detect *who* changed it.
- When an actor retrieves context, a "changes since you last read"-signal surfaces relevant edits.
- The actor acknowledges changes implicitly (next write reflects them) or explicitly (a note in the thread).

This keeps the system honest: any actor can intervene without breaking another's grip on reality.

---

## 7. Main Page / Sub-Page Pattern

Each topic has:
- **One main page** — the current best understanding, kept tight. This is what actors read in daily work.
- **Zero or more sub-pages** — supporting detail, raw material, historical positions, edge cases. Read on dives.

The retrieval API reflects this:
- **Default search** returns main-page content with snippets.
- **Expand / fetch sub-page** is an explicit second step.

Benefits:
- Reading actors keep their context economical — no drowning in detail they didn't need.
- Detail is preserved for verification and dives.
- Forces a quality judgment during distillation: *what belongs on the main page vs. behind it?*

FTS5 indexes everything regardless of layer, so search still finds it.

---

## 8. Distillation: Merge, Append, or Discard

When the evaluator looks at material in L1 (or L2) and decides what to do with it, several outcomes are possible. **There is no fixed rule** — judgment is required — but heuristics:

| Situation | Action |
|---|---|
| Confirms or sharpens existing understanding | **Merge** into main page |
| Adds nuance, edge case, or historical context | **Append** as sub-page |
| Contradicts existing position | **Update main page** with new position; preserve previous as dated sub-page |
| Noise, redundant, low-signal | **Discard** (but L1 source lingers until pruned) |

These actions are entries in the `distillation_actions` lookup table (see §13) — you can add new ones (e.g. *split*, *annotate*, *refactor*) without code changes.

### Triggers for distillation

- **Event-driven:** thread reaches a milestone (question answered, decision made, error resolved).
- **Time-based:** periodic sweep of open threads (interval and inactivity thresholds in config).
- **Threshold-based:** thread has accumulated enough material (length, cross-references — thresholds in config).
- **Explicit:** any actor flags "remember this."

### Guiding principle

*We are afraid of losing information.* When in doubt, keep — but keep it out of the way (sub-page, not main page). Noise is fixable later; lost insight is not.

### Quality control: the asymmetric risk of bad distillation

A bad distiller is the single biggest risk to long-term system health. Small errors in L3 accumulate silently; only careful readers notice, and by then the trust in the substrate is already eroded. Rollback helps only if someone *detects* the contamination — which can take weeks.

The safety mechanism is **proposed vs. stable** for promotions into L3, applied without becoming a gate:

- A distillation into L3 performed by an `agent` or `system` actor lands as a normal edit on the main branch, but is tagged `state=proposed` in the `distillations` table.
- Proposed edits are rendered with a small inline marker in the web UI ("proposed by agent-X, 2 days remaining") so readers see they are looking at unconfirmed material.
- After a configurable maturation period (`distillation.l3_maturation`, default 7 days) with no objection from any actor, the edit auto-transitions to `state=stable`. No human action required.
- Any actor can object during the maturation window by clicking "challenge" on the marker — this reopens the source thread in L1 as a review thread (same mechanism as edit conflicts in §14), and the edit remains `proposed` until resolved.
- Distillations performed by a `human` actor are `stable` immediately — the human is already the reviewer.
- L1→L2 promotions skip this entirely; episodic memory is a log, and noise there is acceptable.

This is consistent with *flow over gates*: nothing is blocked, agents act directly, but the system has built-in time for objection on the only layer where silent contamination is genuinely dangerous.

### Refactoring L3 over time

"Additive by default" risks letting a topic page in L3 become a Frankenstein of ten merged-and-contradicted positions over time. The escape valve is a dedicated **refactor** action in the `distillation_actions` lookup table:

- A refactor distillation consolidates a main page and its sub-pages into a cleaner shape, possibly demoting outdated sub-pages to an `archive/` namespace.
- Refactors are `proposed` by default (same mechanism as above) regardless of actor type, since they touch material that may have been stable for a long time.
- The pre-refactor state is always recoverable via git history.

A refactor is the only operation that *might* shrink the L3 surface area; everything else only adds.

### Anatomy of a distillation record

Distillation is the motor of the system; saying "it happens" is not enough. Every distillation must answer eight questions, and the data model must capture all of them. If any of these is missing, the operation is not traceable, reviewable, or reversible.

| Question | Captured as |
|---|---|
| **What was the source?** | `source_kind` + `source_id` (a thread, a page, a set of L1 entries) — and a denormalized `source_snapshot` (markdown) so the source can be reviewed even if it is later pruned or refactored. |
| **What was written?** | `result_markdown` — the exact markdown produced by this distillation, before any subsequent edits. |
| **Where was it written?** | `target_page_id` and `target_section` (path, optional anchor within the page). |
| **Why this action?** | `action` (`merge`/`append`/`contradict`/`refactor`/`discard`) + `rationale` (markdown — the distiller's reasoning, in its own words). |
| **Who did it?** | `actor_id` (any actor type). |
| **What was preserved?** | `preserved_refs` — list of source elements explicitly carried forward (pointers to L1/L2 items, sub-pages created, quotations retained). |
| **What was discarded?** | `discarded_refs` + `discard_rationale` — what the distiller chose *not* to lift, with a short markdown note per item. This is the hardest thing to inspect post-hoc and the most important thing to record. |
| **How is it reversed?** | `reverse_commit` (the git commit that applied this distillation, so `git revert` is mechanical) + a derived `is_reversible` flag (false only for refactors that crossed a maturation boundary in an irreversible way — rare). |

The `discarded_refs` field is the single most important addition. Bad distillation usually fails by omission, not by commission — by quietly leaving out the nuance, the counter-example, the edge case. If the distiller has to write down *what it deliberately did not lift*, a reviewer can spot a bad call by reading just those notes, without re-reading the entire source.

This is also where humans add the most value as reviewers: not by approving everything, but by spot-checking the *discards*. A bad distiller's discards will read as "this seemed redundant" repeated for material that was actually distinctive.

The schema for this lives in §13 (the `distillations` table is extended accordingly).

---

## 9. Thread State Machine

Threads are the unit of activity. Each has a state visible to all actors.

The state machine is **data-driven**: states and allowed transitions live in the `thread_states` and `state_transitions` tables (see §13), seeded from migrations but editable at runtime. The diagram below is the initial seed, not a hard-coded constraint.

```
  ┌─────────┐    ┌──────────┐    ┌─────────────┐    ┌──────────┐
  │ opened  │ -> │ active   │ -> │ distilling  │ -> │ closed   │
  └─────────┘    └──────────┘    └─────────────┘    └──────────┘
                      │
                      v
                 ┌──────────┐
                 │  paused  │  (long-lived threads can sleep/resume)
                 └──────────┘
```

- A thread may be **short** (a single exchange) or **long-lived** (a working context open for weeks).
- `distilling` is its own state because evaluation is a real operation, not a side effect.
- Transitions can trigger git commits (L2/L3 writes) and real-time events (UI updates).
- Threads carry an `opened_by` actor reference and accumulate participants from any actor type.

Open question: do we model `paused` explicitly, or is "no activity for the configured inactivity threshold" enough?

**Decision:** `paused` is a first-class explicit state. A thread can be idle for many reasons — some "done in spirit, time to close," others "I deliberately set this aside for two weeks." If the system can't distinguish, a `system` distiller will eventually sweep a thread an actor was protecting.

- `paused` threads are **exempt** from automatic distillation sweeps and inactivity-based transitions.
- A `paused` thread carries a small markdown note ("why paused, what to do when resumed") so any actor resuming it (possibly a different one) has context.
- Only an explicit `resume` action moves it back to `active`.
- Inactivity timeouts still apply to `active` threads — that's how we differentiate "forgotten" from "intentionally parked."

---

## 10. Markdown Everywhere — The Universal Format

**Every piece of content in Wikikiki is plain text, optionally with Markdown syntax.** No rich text. No HTML in the content itself. No proprietary formats. No binary blobs masquerading as documents.

### What this means concretely

- **Storage:** all page content is `.md` files on disk. Working-memory values are markdown/text in SQLite.
- **Transmission:** the API delivers markdown source. The web UI renders it client-side or server-side, but the source of truth is the markdown text.
- **Editing:** the web UI's editor is a plain textarea (or markdown-aware editor like CodeMirror) operating on raw markdown. **No WYSIWYG.** The user sees and edits the same characters that get saved.
- **Preview:** a live-preview pane (rendered markdown) sits next to the editor, but the editor field itself is always source.
- **Display:** rendered markdown is shown in read views. Toggle to "view source" reveals the raw markdown.
- **Diffs:** all diffs are text diffs over markdown source. Git understands them natively.
- **Comments, attribution, thread events:** when these appear on a page, they are markdown too (with a small convention like `> — alice (human), 2026-05-22` or a YAML frontmatter block).

### Frontmatter for metadata

Pages may carry a YAML frontmatter block for structured metadata (title, layer, parent, tags, distillation source). The body is markdown. This is the only structured "non-markdown" element, and it is itself plain text:

```markdown
---
title: Distillation
layer: semantic
parent: null
tags: [memory, design]
---

# Distillation

The process of evaluating material in working memory and lifting it to ...
```

Everything an actor (human or AI) needs to know about a page is readable as plain text. No hidden state, no rich-text encoding to decode.

### Why this matters

- **Agents and humans read the same bytes.** No format gap, no lossy conversion.
- **Git diffs stay meaningful.** Reviewing change is human-feasible at every step.
- **Tooling is universal.** Every editor on every platform can edit `.md`.
- **Future-proof.** Plain text outlives every format we could invent on top of it.

This is a principle, not a convenience choice. The web UI is a *renderer* over markdown, not a separate document system.

---

## 11. Technical Stack

| Concern | Choice | Notes |
|---|---|---|
| Language | Rust | Performance + safety; good async story |
| Web framework | `axum` | Mature, ergonomic, good WebSocket/SSE support |
| Database | `sqlx` + SQLite | Compile-time checked queries; FTS5 for search |
| Git | `gix` | Pure Rust, no libgit2 dependency. Shipped in Stage 1; `git2` was the alternative considered |
| Markdown | `comrak` | CommonMark + GFM, fast, configurable |
| Real-time | WebSocket or SSE | SSE simpler if traffic is mostly server→client |
| Frontend | TBD — keep simple (server-rendered + HTMX?) | Avoid heavy SPA until needed |
| Templating | `askama` or `maud` | Compile-time HTML in Rust |

### Why this combination

- **SQLite + FTS5** gives blazing-fast full-text search without a separate service.
- **Git** provides distributed history, sync, and backup almost for free.
- **Markdown** is the universal medium; every actor reads and writes the same text.
- **Rust** keeps the whole thing in one binary, easy to deploy and embed.

### SQLite concurrency notes

SQLite has one writer at a time. For Wikikiki, with potentially many `agent` actors writing to L1, this needs attention from day one:

- **WAL mode** is mandatory (`PRAGMA journal_mode=WAL;`) — enables concurrent readers alongside a writer.
- **Separate connection pools** for readers and writers; writers go through a single connection or a small bounded queue to avoid lock contention.
- **Short transactions.** L1 writes are single-row upserts; never wrap distillation evaluation in a write transaction.
- **`busy_timeout`** set to a few seconds to absorb transient contention.
- **Tested early.** A load test with N simulated agents writing to L1 in parallel should be part of the v1 acceptance criteria — *not* something we discover in production.

SQLite handles surprisingly high concurrent loads when configured this way (well into hundreds of writes/sec). If we ever exceed that, the migration path to PostgreSQL is short because sqlx supports both.

### FTS5 index sync

Page content lives on disk (per §10) but is indexed in `pages_fts`. This is a sync point that can drift if the two get out of step (e.g. content edited directly on disk by an operator, files moved by git operations).

The mechanism:
- Every write through the page API updates both the file and the FTS index in the same logical operation.
- The `pages.content_hash` column (sha256 of file content) is updated on every write. A background `system` task periodically rehashes files and compares; mismatches trigger an FTS reindex and a `system` audit entry.
- An admin endpoint (`POST /admin/fts/reindex`) can force a full reindex; it writes to `admin_audit`.

This is a conservative approach: drift is *expected to happen occasionally* (it's an out-of-band consistency problem), and the recovery is automatic and audited.

### Live-edit vs. restart-required configuration

A pragmatic constraint on the "no hardcoding" principle (§4): not everything that *can* live in the database *should* be edited via the UI in real time.

| Editable at runtime (no restart) | Requires restart |
|---|---|
| Actor types, individual actors | Layer definitions and properties |
| Distillation actions | Thread states and transitions |
| Event kinds | Server bindings, transports |
| Per-actor rate limits | Git implementation choice |
| Subscription target kinds | Database/content paths |

The rule: anything whose *meaning* is woven into running code paths requires a restart, even though it lives in a lookup table. The lookup table form is for *clean evolution between versions*, not for live behavioral mutation.

---

## 12. Repository Layout (initial)

```
wikikiki/
├── Cargo.toml
├── DESIGN.md                  ← this document
├── README.md
├── wikikiki.example.toml      ← reference config file
├── src/
│   ├── main.rs
│   ├── lib.rs
│   ├── config/                ← config loader (TOML + env overrides)
│   ├── db/                    ← SQLite schema, migrations, queries
│   ├── git/                   ← commit/read operations
│   ├── memory/                ← layer-agnostic; specific layers driven by DB
│   ├── distill/               ← evaluator (uses configured actions)
│   ├── thread/                ← state machine (data-driven from DB)
│   ├── web/                   ← axum routes, WS/SSE, templates
│   └── actor/                 ← actor-facing API (agent, external, etc.)
├── migrations/                ← sqlx migrations (including seed data for lookup tables)
├── content/                   ← the wiki itself (markdown files) — path configurable
└── tests/
```

The `content/` location is configurable (`paths.content_root`). Whether it's a submodule, nested repo, or co-located is an operator decision, not a code-level commitment.

---

## 13. SQLite Schema (initial sketch)

**No CHECK constraints on enumerated values.** Anything that might grow new values is a lookup table with a foreign-key reference. Initial values are seeded in migrations but editable at runtime.

```sql
-- ===== Lookup tables (the "vocabulary" of the system) =====

CREATE TABLE actor_types (
    code            TEXT PRIMARY KEY,     -- 'human','agent','system','external'
    label           TEXT NOT NULL,
    description     TEXT,
    is_active       INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE layers (
    code            TEXT PRIMARY KEY,     -- 'working','episodic','semantic'
    label           TEXT NOT NULL,
    description     TEXT,
    git_versioned   INTEGER NOT NULL DEFAULT 0,
    sort_order      INTEGER NOT NULL,
    is_active       INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE thread_states (
    code            TEXT PRIMARY KEY,     -- 'opened','active','paused','distilling','closed'
    label           TEXT NOT NULL,
    description     TEXT,
    is_terminal     INTEGER NOT NULL DEFAULT 0,
    is_active       INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE state_transitions (
    from_state      TEXT NOT NULL REFERENCES thread_states(code),
    to_state        TEXT NOT NULL REFERENCES thread_states(code),
    description     TEXT,
    PRIMARY KEY (from_state, to_state)
);

CREATE TABLE distillation_actions (
    code            TEXT PRIMARY KEY,     -- 'merge','append','contradict','discard'
    label           TEXT NOT NULL,
    description     TEXT,
    is_active       INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE event_kinds (
    code            TEXT PRIMARY KEY,     -- 'message','state_change','distillation', ...
    label           TEXT NOT NULL,
    description     TEXT,
    is_active       INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE subscription_target_kinds (
    code            TEXT PRIMARY KEY,     -- 'thread','page','global'
    label           TEXT NOT NULL,
    is_active       INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE thread_visibilities (
    code            TEXT PRIMARY KEY,     -- 'public','participants','actor_only'
    label           TEXT NOT NULL,
    description     TEXT,
    is_active       INTEGER NOT NULL DEFAULT 1
);

-- Distillation source kinds (was a free TEXT field; now a lookup so queries are robust)
CREATE TABLE source_kinds (
    code            TEXT PRIMARY KEY,     -- 'thread','page','wm_edits','pruned_l1','conflict_resolution','migration'
    label           TEXT NOT NULL,
    description     TEXT,
    is_active       INTEGER NOT NULL DEFAULT 1
);

-- ===== Core tables (reference the lookups by FK) =====

CREATE TABLE actors (
    id              INTEGER PRIMARY KEY,
    type            TEXT NOT NULL REFERENCES actor_types(code),
    name            TEXT NOT NULL,
    handle          TEXT NOT NULL UNIQUE,
    metadata        TEXT,                 -- JSON or markdown frontmatter-style text
    is_admin        INTEGER NOT NULL DEFAULT 0,  -- elevated capability flag (lookup-table edits, token issuance)
    is_active       INTEGER NOT NULL DEFAULT 1,  -- false = revoked; all access denied
    created_at      INTEGER NOT NULL,
    last_seen_at    INTEGER
);

-- Credentials: separate table because one actor may have multiple (password + OAuth, multiple tokens, etc.)
-- and credentials should be deletable independently of the actor row.
CREATE TABLE credentials (
    id              INTEGER PRIMARY KEY,
    actor_id        INTEGER NOT NULL REFERENCES actors(id),
    kind            TEXT NOT NULL,        -- 'password','token','oauth','sso','system_token'
    secret_hash     TEXT NOT NULL,        -- argon2 hash; plaintext shown once at issuance
    label           TEXT,                 -- e.g. "Claude Desktop token", "primary password"
    last_used_at    INTEGER,
    expires_at      INTEGER,              -- nullable; null = no expiry
    revoked_at      INTEGER,              -- non-null = revoked
    created_at      INTEGER NOT NULL
);

CREATE INDEX idx_credentials_actor ON credentials(actor_id, kind);

CREATE TABLE threads (
    id              INTEGER PRIMARY KEY,
    title           TEXT NOT NULL,
    state           TEXT NOT NULL REFERENCES thread_states(code),
    opened_by       INTEGER NOT NULL REFERENCES actors(id),
    visibility      TEXT NOT NULL DEFAULT 'public' REFERENCES thread_visibilities(code),
    visibility_rationale TEXT,            -- required when visibility = 'actor_only'
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    closed_at       INTEGER
);

CREATE TABLE thread_participants (
    thread_id       INTEGER NOT NULL REFERENCES threads(id),
    actor_id        INTEGER NOT NULL REFERENCES actors(id),
    joined_at       INTEGER NOT NULL,
    PRIMARY KEY (thread_id, actor_id)
);

-- Working memory: current state (one row per key, upserted)
-- The naming separates "what is true now" (working_memory) from "what happened"
-- (working_memory_edits and thread_events). Without this split, an upsert would
-- silently erase the audit trail that §3 promises.
CREATE TABLE working_memory (
    id              INTEGER PRIMARY KEY,
    thread_id       INTEGER NOT NULL REFERENCES threads(id),
    key             TEXT NOT NULL,
    value           TEXT NOT NULL,        -- markdown — current value
    written_by      INTEGER NOT NULL REFERENCES actors(id),
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    UNIQUE (thread_id, key)
);

-- Working memory: append-only history of every change.
-- Every upsert to working_memory writes a row here in the same transaction.
-- This is what audit, rollback, and "what has this key been through?" read from.
CREATE TABLE working_memory_edits (
    id              INTEGER PRIMARY KEY,
    thread_id       INTEGER NOT NULL REFERENCES threads(id),
    key             TEXT NOT NULL,
    previous_value  TEXT,                 -- null on first write
    new_value       TEXT NOT NULL,
    actor_id        INTEGER NOT NULL REFERENCES actors(id),
    created_at      INTEGER NOT NULL
);

CREATE INDEX idx_wm_edits_thread_key ON working_memory_edits(thread_id, key, created_at);

CREATE TABLE thread_events (
    id              INTEGER PRIMARY KEY,
    thread_id       INTEGER NOT NULL REFERENCES threads(id),
    actor_id        INTEGER NOT NULL REFERENCES actors(id),
    kind            TEXT NOT NULL REFERENCES event_kinds(code),
    payload         TEXT NOT NULL,        -- markdown (or YAML frontmatter + markdown)
    created_at      INTEGER NOT NULL
);

-- Page metadata; markdown content lives on disk under paths.content_root
CREATE TABLE pages (
    id                  INTEGER PRIMARY KEY,
    path                TEXT NOT NULL UNIQUE,
    layer               TEXT NOT NULL REFERENCES layers(code),
    parent_id           INTEGER REFERENCES pages(id),
    title               TEXT NOT NULL,
    maintainer          INTEGER REFERENCES actors(id),  -- responsible actor for arbitration (any type; null = no designated)
    content_hash        TEXT,                           -- sha256 of file content for FTS sync detection
    created_at          INTEGER NOT NULL,
    updated_at          INTEGER NOT NULL,
    last_edit_at        INTEGER,
    last_edit_by        INTEGER REFERENCES actors(id)
);

CREATE TABLE page_edits (
    id              INTEGER PRIMARY KEY,
    page_id         INTEGER NOT NULL REFERENCES pages(id),
    actor_id        INTEGER NOT NULL REFERENCES actors(id),
    git_commit      TEXT,
    summary         TEXT,                 -- markdown
    created_at      INTEGER NOT NULL
);

CREATE VIRTUAL TABLE pages_fts USING fts5(
    title, content, path UNINDEXED,
    content='', contentless_delete=1
);

-- Distillations: every lift between layers captures the eight questions from §8.
-- The discarded_refs field is critical: bad distillation fails by omission,
-- and only a record of what was deliberately left out lets a reviewer catch it.
CREATE TABLE distillations (
    id                  INTEGER PRIMARY KEY,
    source_kind         TEXT NOT NULL REFERENCES source_kinds(code),
    source_id           INTEGER NOT NULL,
    source_snapshot     TEXT NOT NULL,        -- markdown — the source as it existed at distillation time
    target_page_id      INTEGER NOT NULL REFERENCES pages(id),
    target_section      TEXT,                 -- optional anchor within the target page
    result_markdown     TEXT NOT NULL,        -- the markdown produced by this distillation
    action              TEXT NOT NULL REFERENCES distillation_actions(code),
    rationale           TEXT NOT NULL,        -- markdown — why this action was chosen
    actor_id            INTEGER NOT NULL REFERENCES actors(id),
    preserved_refs      TEXT NOT NULL,        -- JSON array — source elements carried forward
    discarded_refs      TEXT NOT NULL,        -- JSON array of {ref, reason} — what was deliberately left out
    state               TEXT NOT NULL DEFAULT 'stable',  -- 'proposed','stable','challenged','withdrawn'
    matures_at          INTEGER,              -- when proposed → stable transition is due (null = already stable)
    challenged_by       INTEGER REFERENCES actors(id),  -- actor that raised an objection, if any
    challenge_thread_id INTEGER REFERENCES threads(id),  -- review thread spawned by challenge
    reverse_commit      TEXT,                 -- git commit that applied this distillation, for git revert
    is_reversible       INTEGER NOT NULL DEFAULT 1,
    created_at          INTEGER NOT NULL
);

CREATE INDEX idx_distillations_target ON distillations(target_page_id, created_at);
CREATE INDEX idx_distillations_state ON distillations(state, matures_at);

CREATE TABLE subscriptions (
    id              INTEGER PRIMARY KEY,
    actor_id        INTEGER NOT NULL REFERENCES actors(id),
    target_kind     TEXT NOT NULL REFERENCES subscription_target_kinds(code),
    target_id       INTEGER,              -- nullable when target_kind = 'global'
    created_at      INTEGER NOT NULL
);

-- ===== Safety net tables (per §3) =====

-- Admin audit: who changed lookup tables, config, layer definitions, tokens, capabilities.
CREATE TABLE admin_audit (
    id              INTEGER PRIMARY KEY,
    actor_id        INTEGER NOT NULL REFERENCES actors(id),
    target          TEXT NOT NULL,        -- e.g. 'actor_types', 'config:memory.working.prune_after_days', 'credentials:42'
    operation       TEXT NOT NULL,        -- 'insert','update','delete','reload','issue_token','revoke_token'
    before_value    TEXT,                 -- JSON or markdown (never plaintext secrets)
    after_value     TEXT,                 -- JSON or markdown (never plaintext secrets)
    note            TEXT,                 -- markdown
    created_at      INTEGER NOT NULL
);

-- Access log: login attempts (success and failure), access denials, suspicious patterns.
-- Separate from admin_audit because it has different retention and access rules.
CREATE TABLE access_log (
    id              INTEGER PRIMARY KEY,
    actor_id        INTEGER REFERENCES actors(id),  -- null when login attempt fails before resolution
    attempted_handle TEXT,                -- the handle/username that was tried (for failed logins)
    surface         TEXT NOT NULL,        -- 'ui','api','mcp'
    outcome         TEXT NOT NULL,        -- 'login_success','login_failure','denied_read','denied_write','token_used'
    target          TEXT,                 -- e.g. 'page:semantic/foo', 'thread:42'
    remote_addr     TEXT,                 -- hashed or pseudonymized per privacy policy
    created_at      INTEGER NOT NULL
);

CREATE INDEX idx_access_log_actor_time ON access_log(actor_id, created_at);
CREATE INDEX idx_access_log_outcome ON access_log(outcome, created_at);

-- Tombstones: soft-deleted content waiting for retention window expiry
CREATE TABLE tombstones (
    id              INTEGER PRIMARY KEY,
    source_kind     TEXT NOT NULL,        -- 'page','working_memory','thread_event'
    source_id       INTEGER NOT NULL,
    snapshot        TEXT NOT NULL,        -- full markdown content at deletion
    deleted_by      INTEGER NOT NULL REFERENCES actors(id),
    deleted_at      INTEGER NOT NULL,
    expires_at      INTEGER NOT NULL,     -- after this, hard delete is allowed
    recovered_at    INTEGER,              -- if non-null, was restored
    recovered_by    INTEGER REFERENCES actors(id)
);

-- ===== Seed data (migrations) =====
-- Migrations populate actor_types, layers, thread_states, state_transitions,
-- distillation_actions, event_kinds, and subscription_target_kinds with the
-- initial values described elsewhere in this document. None of these values
-- are referenced by literal in application code; everything reads them from DB.
```

The result: adding a new actor type, layer, state, transition, or distillation action is a row insert, not a code change.

---

## 14. Web Interface

The web UI is the primary observation and correction surface. It is not a thin wrapper over the database — it is the lens through which actors (especially `human` ones) see live memory and intervene.

**The UI is a markdown renderer + a markdown editor.** Nothing more conceptually. Everything the user sees is either rendered markdown (read mode) or raw markdown source (edit mode). There is no rich-text layer, no document model on top of the markdown — see §10.

### Access and identity — the foundation

The web UI does not have a separate "user system". It is one of three surfaces (web UI, REST API, MCP server) onto the same `actors` table. Identity, access, and accountability work the same across all three surfaces — the UI is just the one humans use.

#### Identity rules

Identity is bound to a credential at issuance time. An actor cannot change their own type or impersonate another actor.

- **`human` actors** authenticate to the web UI via session cookie. Credentials (password, OAuth, SSO assertion) resolve to a row in `actors` with `type='human'`. The session cookie carries the actor ID.
- **`agent` and `external` actors** authenticate via API token (used over REST or MCP). Tokens are issued by an admin operation that creates the actor row first and binds the token to it. The token's actor type is fixed at issuance — no API call can promote an `agent` to `human`.
- **`system` actors** authenticate via a service token loaded from environment variables at boot, never persisted in the DB.
- Every request — UI or API — resolves to exactly one actor before any handler runs. Handlers see `(actor_id, actor_type)` and use it for attribution, rate limiting, and access checks.
- Tokens carry no claims beyond actor identity; permissions are looked up server-side at request time, so revoking an actor immediately revokes their access.
- **Token storage:** hashed (argon2 or similar); plaintext shown once at issuance.
- All token operations (issue, revoke, reissue) write to `admin_audit` (§3.1).

OAuth/SSO for `human` actors is a thin layer on top — the OAuth response resolves to a `human` actor row, the same as a password login would.

#### Access modes

Three coarse modes govern what unauthenticated visitors can do, all configurable:

| Config | Effect |
|---|---|
| `ui.allow_anonymous_read = true` (default) | Anyone can read `public` content without logging in. |
| `ui.allow_anonymous_read = false` | The UI redirects to login on every request. No read access without an actor. |
| `ui.require_auth_for_write = true` (default) | Any write requires an authenticated actor. |

Anonymous readers do not have an `actor_id`; their reads are not attributed. They cannot subscribe, edit, or open threads.

#### Access rules — who can do what

Access is governed by a small set of rules that apply uniformly across UI, REST API, and MCP. There is no separate UI permission system.

**Reading:**

| Resource | Anonymous (if allowed) | Any authenticated actor | Participant / subscriber | Maintainer / opener |
|---|---|---|---|---|
| `public` page (any layer) | ✓ | ✓ | ✓ | ✓ |
| `public` thread | ✓ | ✓ | ✓ | ✓ |
| `participants` thread | ✗ | ✗ | ✓ | ✓ |
| `actor_only` thread | ✗ | ✗ | ✗ | ✓ (opener only) |
| Actor profile page | ✓ | ✓ | ✓ | ✓ |
| Git history of a page | inherits page access | inherits page access | inherits page access | inherits page access |
| Admin audit log | ✗ | ✗ | ✗ | ✗ (admin role only) |

**Writing:**

| Action | Who can do it |
|---|---|
| Edit a `public` page | Any authenticated actor (subject to rate limits) |
| Edit a `participants` thread | Only listed participants |
| Edit an `actor_only` thread | Only the opening actor |
| Open a new thread | Any authenticated actor |
| Set a thread's `visibility` | The opener, or the page maintainer if attached to a page |
| Set a page's `maintainer` | The current maintainer; or any `human` actor if `maintainer` is null |
| Issue / revoke tokens | Actors with the `admin` capability flag (see below) |
| Edit lookup tables, config | Actors with the `admin` capability flag |
| Edit another actor's profile *page* (narrative markdown) | Any `human` actor (it's just a wiki page) |
| Edit an actor's identity row (type, handle, capabilities) | Admin only — and the change writes to `admin_audit` |

**Admin capability:** a boolean flag on the `actors` row (`is_admin`), seeded at install for the first `human` actor and granted by another admin thereafter. Admin is *not* an actor type — admins are still `human` (or rarely `system`); the flag adds privileges without changing the modeling category.

#### Access enforcement principles

- **Enforced server-side, always.** UI affordances may hide buttons, but the server re-checks on every action. The UI is not a security boundary.
- **Visibility downgrades on changes.** If a thread's visibility changes from `public` to `participants`, current readers without participant status lose access immediately. (The change itself is in `thread_events` and visible via audit.)
- **Failed access attempts are events.** A denied read or write is logged to `admin_audit` with the attempting actor and target — so probing patterns are observable.
- **`system` actors bypass most access rules** but cannot exceed the rate limits set for `system`, and every `system` action is attributed. They cannot read `actor_only` threads belonging to other actors — that's a hard rule, not just a default.

### Design principles

- **Live by default.** Open a page, see it update as it changes. No refresh button.
- **Read and write in the same place.** Editing is inline; no separate "edit mode" modal where possible. Clicking a section flips it from rendered to raw markdown.
- **Markdown is what you see and what you save.** No WYSIWYG. The textarea content is the file content.
- **Layers are visible but unobtrusive.** A small badge per page indicates which layer; sub-pages are visually folded.
- **Attribution everywhere.** Every change shows the actor (with type badge) and time, rendered inline as part of the markdown flow (e.g. as a blockquote line).
- **Access affordances follow access rules.** Buttons for actions an actor cannot perform are hidden, not greyed out — to avoid teasing capabilities the actor doesn't have.

### Core views

| View | Purpose |
|---|---|
| **Dashboard** | Active threads, recent activity, who's online, current global state at a glance. |
| **Thread view** | One thread: state, working memory entries (as markdown), event log, participants. Updates live. |
| **Page view** | A markdown page with its sub-pages, edit history, and "changes since" markers. |
| **Live memory** | Raw working-memory dump per thread or actor — rendered as markdown. |
| **Search** | FTS5-powered full-text across all layers, with filters by layer, actor, time. |
| **Actor profile** | A markdown page per actor (auto-generated, editable) showing what they write and watch. |
| **Diff / history** | Per-page git history with text diffs on the markdown source. |

### Editing

- **Inline edit:** click a section → it turns into a textarea with the raw markdown → save commits to git + updates FTS index. Esc cancels.
- **Editor:** plain textarea or CodeMirror with markdown syntax highlighting. *No* WYSIWYG, *no* rich-text toolbar that hides the syntax.
- **Live preview:** optional split pane shows rendered markdown alongside the source. Toggleable.
- **Conflict handling:** the strategy is deliberately simple: **last-writer-wins on commit, with a visible review thread for the loser's version.**
  1. Both edits proceed independently in their own editor sessions; there is no live collaborative editing layer (no CRDT, no operational transform) in v1.
  2. The first write to land commits normally on the main branch.
  3. The second write is committed to a **temporary branch** named for the actor and timestamp (e.g. `conflict/agent-researcher/2026-05-22T14-32`).
  4. A **review thread** is opened automatically in L1, visible to all watchers, with a defined set of responsible actors (see "Who arbitrates" below) and a fallback if no one acts.
  5. Resolution writes the merged page back to main and closes the review thread; the branch is preserved in git history (but periodically gc'd — see below).
  6. *Mitigation against the loser losing work invisibly:* the editor shows a non-blocking "this page changed under you" banner the moment a server-pushed update arrives, with a one-click "see the other version" link.
  
  This is consistent with *additive by default* (no edit is destroyed) and *visible conflicts* (no silent loss). Live collaborative editing (CRDT) is a possible future enhancement, not a v1 commitment.

#### Who arbitrates

A review thread without a responsible actor will rot. The default arbiter set, in priority order:

1. The page's **`maintainer`** if set (a column on `pages` — can be any actor type; `null` by default).
2. Actors who have **edited the page** in the last 30 days.
3. Actors **subscribed** to the page or its parent.
4. If none of the above exist or respond within `conflict.stale_timeout` (default 3 days), a `system` actor takes a **conservative fallback**: keep the version already on main, archive the conflict branch with a note in the review thread. The thread is closed, but the branch's commit hash is preserved in the thread's markdown snapshot, so the discarded version can still be recovered manually.

The fallback is deliberately conservative — it never *picks* between two contested versions; it just prevents indefinite open threads.

#### Branch hygiene

Over time, conflict branches accumulate. Policy:
- Conflict branches with a closed review thread and no activity for `git.conflict_branch_retention` (default 30 days) are deleted by a `system` job.
- The review thread always carries the markdown snapshot of the discarded version (committed at thread open), so branch deletion never loses content — only commit ancestry.
- A `system` distillation entry is written at deletion time with `action='discard'` and rationale "conflict branch retention expired", consistent with §5's no-silent-loss rule.
- **Soft delete:** content moves to a configured `deleted/` namespace with a tombstone (still markdown), recoverable. Hard delete is a separate operation.
- **Images and binaries:** stored as files alongside the markdown, referenced from markdown with standard `![alt](path)` syntax. The markdown remains plain text.

### Real-time transport

Configured via `realtime.transport` (`sse`, `websocket`, or `both`):
- **SSE** by default: one-way server → client, simple, proxy-friendly.
- **WebSocket** when interaction needs bidirectionality (presence, typing indicators, collaborative cursors).
- **Subscriptions** are explicit: a client subscribes to threads, pages, or "all activity"; the server pushes deltas.

### Tech choices for the UI

- **Server-rendered HTML** with `askama` or `maud` for templates. Fast, simple, no build pipeline.
- **HTMX** for interactivity without a SPA — SSE + HTMX swap = live regions with very little JS.
- **Small islands of vanilla JS** (or Alpine) for editor widgets and keyboard shortcuts.
- **No heavy SPA framework** until concrete need emerges.

### URL structure (sketch)

```
/                          → dashboard (requires auth unless allow_anonymous_read)
/login                     → login form (password / OAuth)
/logout                    → end session
/threads                   → list of threads (filtered by visibility + access)
/threads/:id               → one thread (live)
/wiki/*path                → page by path (rendered markdown)
/wiki/*path?edit=1         → page in edit mode (raw markdown)
/wiki/*path?source=1       → page source view (read-only raw markdown)
/wiki/*path/history        → git history
/wiki/*path/sub            → sub-pages index
/episodic                  → episodic log browser
/search?q=...&layer=...    → search (results filtered by access)
/actors                    → actor directory
/actors/:handle            → actor profile (the markdown page; identity row is separate)
/me                        → current actor's profile + token management
/live                      → global live feed (everything happening now)
/sse/threads/:id           → SSE stream for one thread
/sse/global                → SSE stream of everything
/ws                        → WebSocket endpoint (when configured)

# Admin (requires is_admin)
/admin                     → admin dashboard
/admin/actors              → create/edit actors, issue tokens, toggle is_admin
/admin/lookups             → edit lookup tables
/admin/audit               → admin_audit log viewer
/admin/access              → access_log viewer (denied attempts, suspicious patterns)
/admin/fts/reindex         → force FTS5 reindex (writes admin_audit)
```

URL paths and bind address are configurable; the structure above is the default shape.

### Actor API parallel

The actor API (used by `agent`, `external`, and `system` actors) mirrors the web UI's resources but returns markdown directly. **There is no separate JSON document format for content** — content is markdown. Structured wrappers (envelopes with IDs, timestamps, attribution) are JSON, but the payload they wrap is always markdown text.

```
GET    /api/threads/:id                  → JSON envelope, markdown payloads
POST   /api/threads/:id/events           → body: markdown (+ optional kind)
PUT    /api/working_memory/:thread/:key  → body: markdown
GET    /api/pages/:path                  → markdown source (or JSON envelope on Accept header)
PUT    /api/pages/:path                  → body: markdown
POST   /api/distillations                → JSON metadata, markdown note
GET    /api/search?q=...                 → JSON results, markdown snippets
```

Authentication and access rules for the REST API are identical to those described at the start of this section — token-bearing requests resolve to an `actors` row, and all reads and writes go through the same access checks as UI requests.

### MCP (Model Context Protocol) interface — first-class

Wikikiki ships an **MCP server** alongside the REST API. This is not optional or "maybe later" — it is the primary way external agents (Claude Desktop, terminal agents, autonomous runners, third-party clients) attach to a Wikikiki instance.

The reasoning: if Wikikiki speaks MCP, then any MCP-compatible client can use it as a tool with zero bespoke integration. Building Wikikiki-specific adapters for every new agent model defeats the point of having a universal substrate.

**Tools exposed over MCP (initial set):**

- `wikikiki_search(query, layer?, limit?)` — full-text search across pages.
- `wikikiki_read_page(path)` — fetch a page (main + listing of sub-pages).
- `wikikiki_read_subpage(path)` — fetch a specific sub-page.
- `wikikiki_write_page(path, markdown, summary)` — create or update a page.
- `wikikiki_append_subpage(parent_path, markdown, summary)` — add a sub-page.
- `wikikiki_open_thread(title, initial_note?)` — open a new thread.
- `wikikiki_thread_event(thread_id, kind, markdown)` — append an event to a thread.
- `wikikiki_working_memory_set(thread_id, key, markdown)` — write to L1.
- `wikikiki_working_memory_get(thread_id, key?)` — read from L1.
- `wikikiki_propose_distillation(source, target, action, note)` — flag material for distillation.

**Resources exposed over MCP:**

- The wiki tree as browsable resources (`wiki://path/to/page`).
- Active threads as resources (`thread://id`).
- Live working memory as resources, so an MCP client can subscribe to it.

The MCP server is just another transport over the same underlying actor API — **same authentication model, same access rules, same data model, same markdown payloads.** An MCP client identifies as an `agent` (or `external`) actor and shows up in attribution like any other. Every MCP tool call goes through the access checks defined at the start of this section; an `agent` actor cannot read an `actor_only` thread via MCP that it could not read via the REST API.

This means an agent running in a terminal, in Claude Desktop, or as a long-running autonomous process all interact with the same wiki through the same protocol — and a human watching the web UI sees their writes appear in real time, attributed correctly.

---

## 15. Multi-Actor Operation

Multiple actors share **one** wiki: one semantic layer, one episodic log, one substrate. This is the whole point — actors *together* are the system. But threads need a visibility model so that an agent's internal scratch work doesn't clutter every observer's view.

### Thread visibility

Every thread carries a `visibility` value:

| Value | Meaning | Default for |
|---|---|---|
| `public` | Any actor can read; any actor can write (subject to rate limits and auth). Surfaces in dashboards and live feeds. | Most threads — including auto-opened review threads. |
| `participants` | Only actors in `thread_participants` can read or write. Visible in participants' personal views; absent from global feeds. | Manually-opened threads where one actor wants to confine the conversation. |
| `actor_only` | Only the opening actor can read/write. A scratchpad. Never surfaces anywhere else. | Explicit opt-in — must be set at thread open, with a `rationale` string captured in the thread metadata. |

The default for an auto-opened thread (review, distillation challenge, system action) is `public`. The default for an actor-opened thread is `participants` with the opener as sole initial participant, who can then invite others or escalate to `public`.

**`actor_only` is deliberately friction-y.** Requiring a rationale at open time is not a gate — the thread opens immediately — but it makes private scratch space a visible, attributed choice rather than a quiet default. This is consistent with *observable always* as a core principle: privacy exists, but it is itself observable.

### Symmetry across actor types

Participation features (presence indicators, typing signals, "currently viewing") are **symmetric** across actor types by default. An agent that connects via MCP and is actively reading a page shows up the same way a human does. This is not just modeling purity — it lets humans see *what agents are looking at right now*, which is itself valuable observability.

### Per-actor namespaces

Working-memory keys are scoped per `(thread_id, key)`, not per actor. Multiple actors writing to the same key in the same thread is a *deliberate collaboration*, and the edit history (§5) shows who wrote what when. If an actor wants private working memory, they open an `actor_only` thread.

### Other resolved questions

- **Per-actor attribution on writes:** yes — every write carries `actor_id`.
- **Private scratch space:** yes, via `actor_only` threads, with rationale.
- **`human` actors getting richer features:** no — symmetry by default. Differences only emerge in transport (UI vs. API), not in capabilities.

### Operating an agent against Wikikiki (the thinking loop)

For an `agent` actor to actually *use* Wikikiki as its memory — rather than just write logs to it — the orchestrator that boots the agent must instruct it to externalize its cognitive state into L1. The Wikikiki schema makes this attractive (live observability, automatic attribution, current-state semantics via `UNIQUE (thread_id, key)`), but the *behavior* must come from the agent's system prompt. Wikikiki provides the substrate; the orchestrator provides the discipline.

The pattern, in three lines:

1. **No durable internal memory between runs.** Cognitive state — current hypothesis, task backlog, checkpoints — lives in `working_memory`, not in the agent's process.
2. **State as keys, not logs.** The agent maintains a small set of semantically named markdown values, updated in place. The audit trail comes from `working_memory_edits`; the agent doesn't have to construct it.
3. **Resume by reading.** First action on waking in a thread is to read existing `working_memory` for that thread. If it isn't in Wikikiki, it didn't happen.

A reference system prompt template implementing this lives outside DESIGN.md (see `docs/agent-prompt-template.md`) so it can evolve without touching the architecture document. Key names like `current_hypothesis`, `task_backlog`, `checkpoint_data` are *conventions* — useful defaults, not architectural commitments. Deployments are free to use their own vocabulary.

#### Concrete example: a debugging agent in thread #142

**Step 1 — Context restore.** The agent wakes and calls the MCP tool `wikikiki_working_memory_get(thread_id=142)`. The server returns the current state:

```json
[
  {
    "key": "current_hypothesis",
    "value": "### Hypothesis\nSuspect tokio tasks aren't being dropped correctly in the parser thread under load."
  },
  {
    "key": "checkpoint_data",
    "value": "### Last checkpoint\n- File analyzed: server.log.3\n- Last line processed: 45002\n- Error rate: 2.4%"
  }
]
```

**Step 2 — Work, then mutate state in place.** The agent investigates, decides the hypothesis was wrong, and writes the new one to the same key:

```json
{
  "tool": "wikikiki_working_memory_set",
  "arguments": {
    "thread_id": 142,
    "key": "current_hypothesis",
    "value": "### Hypothesis (updated)\nTokio tasks drop fine. The leak is in the sqlx connection pool — `max_connections` is too low during migration, causing the pool to exhaust under load."
  }
}
```

The write hits the `UNIQUE (thread_id, key)` constraint and upserts. Three things happen as a consequence:

- The new value becomes the current state — readable by any other actor (human or agent) in real time.
- The previous value is preserved in `working_memory_edits`, with attribution to this agent.
- Subscribers see an SSE event; a human watching the UI sees the hypothesis change live, with the agent's badge next to it.

The agent can now exit — whether deliberately or because its run ended — knowing the next agent (or human) to enter the thread reconstructs the full picture by reading `working_memory`. There is no "agent state" to lose because there was never any agent state to begin with.

#### Why this validates the schema

This example is the concrete reason `working_memory` has `UNIQUE (thread_id, key)`. The table is not a dumping ground for text — it is a **live state machine for the agent's cognition**, where each key is a named cognitive slot the agent maintains over time. The split between current state and history (§5, §13) exists precisely because the agent needs *one obvious place to look* for "what do I currently think?", while the system still preserves how that thought evolved.

If `working_memory` had been append-only, the agent would have to scan and reconstruct on every wake — slow, error-prone, and unfriendly to humans observing live. If it had been versioned in place without `working_memory_edits`, history would silently vanish on every upsert. The schema is what it is because this pattern is what it has to support.

---

## 16. Open Questions / To Decide

Resolved (moved into the relevant sections):
- ~~`paused` thread state~~ → §9: explicit first-class state.
- ~~MCP server~~ → §14: ship from day one alongside REST.
- ~~Concurrent edit conflicts~~ → §14: git temporary branch + auto-review thread.
- ~~Arbiter role~~ → §14: maintainer → recent editors → subscribers → conservative `system` fallback after `conflict.stale_timeout`.
- ~~`source_kind` in distillations~~ → §13: now a lookup table (`source_kinds`).
- ~~L1 pruning as silent reaper~~ → §5: pruning now writes `system` distillations with `discard` action.
- ~~Multi-actor working memory visibility~~ → §15: `visibility` per thread (`public`/`participants`/`actor_only`), `public` by default.
- ~~Loop detection across actors~~ → §3.5: edit-churn detection with configurable thresholds.
- ~~Conflict branch garbage~~ → §14: 30-day retention after closed review thread; markdown snapshot preserved in the thread.
- ~~`gix` vs `git2`~~ → §11: `gix` shipped in Stage 1 and covers what that stage needs — commit, tree editing, history walk, reading a blob at a commit. §14's conflict branches need branch creation and merge, which Stage 4 must confirm `gix` handles before committing to it.

Still to decide:

- **`content/` repo strategy:** submodule, nested repo, or same repo with conventions? (Path is configurable; this is an operator decision but a recommended default would help.)
- **Commit cadence to L2:** `on_state_change` is the default; do we need `batched` from day one or can it wait?
- **OAuth/SSO integration:** API tokens for non-`human` and cookies for `human` are confirmed. When (and which providers) for SSO?
- **Sub-page granularity:** one sub-page per dive, or accumulate in a single `notes.md` per topic? Probably a per-deployment style choice; needs a recommended default.
- **Actor identity for distillation:** when a `system` distiller acts, does it get one generic `system:distiller` identity or one per policy (e.g. `system:l1-pruner`, `system:l3-maturer`)?
- **`actor_only` rationale enforcement:** should the system reject `actor_only` threads without a rationale, or warn-and-allow?

---

## 17. Staged Delivery Plan

The full design above is a north star, not a v1 scope. Building everything at once is the surest way to discover that one of the hard parts (concurrent edits, real-time sync, distillation quality, multi-actor coordination) breaks an assumption that everything else depended on.

The plan is to build in stages, each of which is a working system on its own.

### Stage 1 — Markdown wiki with git backing

- Markdown files on disk, versioned in git.
- A minimal web UI: page view, page edit, page history, search (FTS5 via SQLite, even though most other DB tables aren't used yet).
- **The full access foundation: `actors`, `credentials`, `access_log`, `admin_audit` tables; login/logout; token issuance; `allow_anonymous_read` and `require_auth_for_write` behavior; access rules enforced on every read and write.**
- Two actor types: `human` (cookie session) and `agent` (API token), both writing to the same wiki.
- Attribution via git commit author + a `page_edits` table.
- No L1, no distillation, no state machine, no real-time push.

**Stage 1 proves:** the actor model + access foundation works end to end; git is a viable substrate; markdown editing is good enough for humans and agents both. Critically, access is *not* an add-on later — it ships in v1 because everything else relies on it.

### Stage 2 — Live working memory (L1) and real-time observation

- Add `working_memory`, `threads`, `thread_events` tables and the thread state machine.
- SSE-based real-time push to the web UI.
- A "live memory" view that shows L1 updating in real time.
- Cross-actor edit signals (last_edit_at, changes-since-you-last-read).
- MCP server exposing the working-memory and page read/write tools.

**Stage 2 proves:** the substrate works as live shared memory; humans can observe agents in flow; real-time UI is feasible without an SPA.

### Stage 3 — Distillation

- The full L1 → L2 → L3 promotion machinery.
- Distillation triggers (event, time, threshold, explicit).
- Proposed-vs-stable promotion for L3 with maturation window.
- Sub-page pattern for L3.
- Refactor action.

**Stage 3 proves:** the memory gradient holds up under real use; distillation quality is acceptable; L3 doesn't degrade into noise.

### Stage 4 — Hardening and scale

- Conflict-resolution branches and review threads (with arbiter cascade).
- Soft delete / tombstones with retention.
- Rate limits, duplicate suppression, loop detection.
- Thread visibility levels (`participants`, `actor_only`).
- Admin audit, rollback UI, FTS sync verification.
- SSO/OAuth integration if needed.
- Load testing with many concurrent agents.

**Stage 4 proves:** the safety net is real; the system survives adversarial or buggy agents.

### What every stage shares

Even Stage 1 ships with: the full actor model, markdown-everywhere, git versioning, attribution, audit. These are the foundation; nothing later is allowed to violate them.

What every stage *defers*: anything that isn't on its critical path. Stage 1 doesn't need L1 or distillation, so it doesn't have them. Stage 3 doesn't need conflict-resolution branches yet (Stage 1 and 2's last-writer-wins is sufficient until distillation introduces more complex contention). Don't pay the complexity tax before you need to.

---

## 18. Guiding Principles (the constitution)

1. **The wiki is the memory.** Not a sidecar, not a log — the actual substrate the actors think in.
2. **A memory gradient exists.** Live → episodic → semantic. The shape is data; the existence is product.
3. **Distillation is a first-class operation.** Evaluation of what to remember is not a side effect — it is the engine.
4. **All actors are equal in the model.** `human`, `agent`, `system`, `external` are values, not branches.
5. **One access model across all surfaces.** Web UI, REST API, and MCP share identity, authentication, and access rules. The UI is not a separate permission system.
6. **Markdown is the universal format.** All content is plain text or markdown. The web UI renders over it, not on top of it.
7. **Flow over gates, with a real safety net.** Actors act directly; audit, rollback, and visible conflicts catch what gates would have stopped.
8. **No hardcoding of the shell.** Implementation details (thresholds, vocabularies, paths) live in config or lookup tables. But core principles do not — they are fixed.
9. **Additive by default.** Lifting material never destroys the source. Information loss is the failure we fear most.
10. **Cross-actor edits are events.** Any actor notices and adapts when others change shared state.
11. **Observable always.** If it's in memory, it can be seen — and attributed to the actor who put it there.

---

*Last updated: 2026-05-22 — initial draft.*
