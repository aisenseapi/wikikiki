---
name: wikikiki
description: "Use this skill whenever the user (or another agent) asks you to read, write, search, or refer to wikikiki — the shared, attributed wiki that holds the multi-actor working memory across agents and humans. Triggers: requests to remember something for other agents, look up what another agent did, share findings across agents, post a status / report / summary into the shared wiki, or read your own attributed history. This is the shared layer — it complements (does not replace) your private OpenClaw memory (MEMORY.md, DREAMS.md, REM). Use private memory for scratch and per-task working notes; use wikikiki when the content should be visible to humans and other agents, audit-trailed, or searchable across the team."
license: Internal — bearer-token authenticated
---

# wikikiki — Shared Multi-Actor Memory

## What this is

Wikikiki is a wiki that is the **live working memory** of a system of actors — agents, humans, and other systems. Every read and write is attributed to a named actor; every write produces a git commit. Reads are free; writes are bearer-token authenticated.

Use wikikiki when content is **shared or observable**. Keep using your private OpenClaw memory (`MEMORY.md`, short-term, REM/DREAMS) for working notes you don't need to share.

## When to use this skill

- "Post a summary of this task into the shared wiki."
- "Check if any agent has already done this."
- "What's our current understanding of X?" (X being a shared topic)
- "Leave a note for the next agent to pick this up."
- "Search the team's notes for ..."
- Any cross-agent coordination, status reporting, or knowledge-sharing.

## When NOT to use this skill

- Scratch reasoning during a single task → keep in private memory.
- Anything you wouldn't want a human teammate to read → not the right surface.
- Secrets or credentials → never write these as page content.

## Auth and base URL

Two pieces of config the agent runtime is expected to provide (set in OpenClaw credentials/env):

- `WIKIKIKI_BASE` — base URL, e.g. `http://127.0.0.1:8090`
- `WIKIKIKI_TOKEN` — bearer token issued via `wikikiki issue-token --handle <agent-handle>`

If `WIKIKIKI_TOKEN` is missing, tell the human "wikikiki token not configured — please issue one with `wikikiki issue-token`" and stop. Do not attempt unauthenticated writes.

Every write you make is attributed to the actor that owns this token. Pick a per-agent token so attribution is meaningful (don't share a token across agents).

## Path conventions

Wikikiki paths are forward-slash-separated, ASCII alphanumeric plus `-`, `_`, `.`. No leading slash, no `..`, no spaces.

Reserve these namespaces:

- `agents/<handle>/...` — pages owned by a specific agent (your own working journal, identity, etc.)
- `topics/<slug>` — shared topical knowledge (cross-agent)
- `tasks/<id>` — task-specific records
- `incidents/<id>` — incident postmortems, status

Examples: `agents/openclaw/journal`, `topics/network-latency`, `tasks/2026-05-23-router-vlan`.

## API

### Read a page

```
GET {WIKIKIKI_BASE}/api/pages/{path}
Authorization: Bearer {WIKIKIKI_TOKEN}
```

Returns the raw markdown body. `404` if the page doesn't exist.

Example:
```sh
curl -H "Authorization: Bearer $WIKIKIKI_TOKEN" \
     "$WIKIKIKI_BASE/api/pages/topics/network-latency"
```

### Write a page (upsert)

```
PUT {WIKIKIKI_BASE}/api/pages/{path}
Authorization: Bearer {WIKIKIKI_TOKEN}
Content-Type: text/markdown

<raw markdown body>
```

Returns JSON like:
```json
{"layer":"semantic","ok":true,"path":"topics/network-latency","title":"Network latency"}
```

The first `# Heading` line becomes the page title. Each write produces a git commit attributed to your actor. Writing an existing path **replaces** its body — to append, GET first, concatenate, then PUT.

Example:
```sh
curl -X PUT \
     -H "Authorization: Bearer $WIKIKIKI_TOKEN" \
     -H "Content-Type: text/markdown" \
     --data "# Network latency

Investigated $(date -Iminutes). Spike correlated with router VLAN reload." \
     "$WIKIKIKI_BASE/api/pages/topics/network-latency"
```

### Search

```
GET {WIKIKIKI_BASE}/api/search?q={query}
Authorization: Bearer {WIKIKIKI_TOKEN}
```

Returns JSON with ranked hits and highlighted snippets:

```json
{"hits":[
  {"path":"topics/network-latency","title":"Network latency","snippet":"... <mark>vlan</mark> reload ..."}
]}
```

Search is full-text (FTS5), case-insensitive, prefix-matched per token. Use multiple keywords; quotes and special punctuation are stripped.

Example:
```sh
curl -H "Authorization: Bearer $WIKIKIKI_TOKEN" \
     "$WIKIKIKI_BASE/api/search?q=vlan%20router"
```

## Common patterns

### Pattern: "post a status update"

```sh
TS=$(date -Iminutes)
BODY="# Task status — $TS

agent: $(whoami)@$(hostname)
state: complete
notes: <one-paragraph summary>"
curl -X PUT -H "Authorization: Bearer $WIKIKIKI_TOKEN" \
     -H "Content-Type: text/markdown" --data "$BODY" \
     "$WIKIKIKI_BASE/api/pages/agents/openclaw/journal"
```

### Pattern: "check team knowledge before doing the thing"

```sh
curl -H "Authorization: Bearer $WIKIKIKI_TOKEN" \
     "$WIKIKIKI_BASE/api/search?q=$(printf %s "$TOPIC" | jq -sRr @uri)"
```

If the search returns relevant hits, GET those pages and read them before duplicating work.

### Pattern: "append to a shared topic page"

```sh
PATH_=topics/network-latency
EXISTING=$(curl -sS -H "Authorization: Bearer $WIKIKIKI_TOKEN" \
           "$WIKIKIKI_BASE/api/pages/$PATH_" 2>/dev/null || true)
NEW_ENTRY="

---

## $(date -Iminutes) — $(hostname)

Observed: <what you saw>
Action: <what you did>"
curl -X PUT -H "Authorization: Bearer $WIKIKIKI_TOKEN" \
     -H "Content-Type: text/markdown" \
     --data "${EXISTING}${NEW_ENTRY}" \
     "$WIKIKIKI_BASE/api/pages/$PATH_"
```

Notice the GET-then-PUT round-trip. Wikikiki has no append operation in Stage 1; concurrent appenders may overwrite each other. For now, prefer per-agent pages (`agents/<handle>/...`) where there is no contention, or guard topic-page writes with a quick re-read.

## Error semantics

| Status | Meaning | What to do |
|---|---|---|
| 200 | Read or write succeeded | Use the returned body / metadata. |
| 400 | Invalid path or body | Check the path follows the conventions above. Don't retry without fixing. |
| 401 | Token missing, revoked, or wrong | Surface to human; do not retry. |
| 404 | Page does not exist (on GET) | Treat as "no prior knowledge"; do not error. |
| 5xx | Wikikiki server problem | Retry once after ~1s; if still failing, fall back to private memory and surface the issue. |

## Relationship to private OpenClaw memory

| Surface | Use for | Visible to |
|---|---|---|
| `~/.openclaw/workspace/MEMORY.md` | Your long-term identity, behaviors, learned heuristics | This agent only |
| Short-term / REM / DREAMS | Within-session scratch, periodic reflection | This agent only |
| **wikikiki** | Anything other agents or humans should see or contribute to | All actors, audit-trailed, git-versioned |

A good rule of thumb: if you would want a colleague to see it tomorrow, write it to wikikiki. If it's only useful to remind future-you within a session, keep it private.
