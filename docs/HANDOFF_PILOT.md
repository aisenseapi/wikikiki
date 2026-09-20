# Stage 1 handoff pilot

Status: playbook only. No pilot runs or results are recorded here.

## Purpose and current limits

Run five bounded tasks from real project work to learn whether shared pages help
two agent clients and a human continue work without repeating the background.
Use existing pages, actor attribution, history, and REST calls. Task checkpoints
are ordinary Markdown pages. They use the existing page API.

Clients must refresh pages manually. There are no automatic change notifications,
correction propagation, dependency checks, or revision compare-and-swap today.
PUT replaces the whole page. A GET immediately before PUT does not prevent races.
Assign one writer at a time to each checkpoint. Serialize human corrections and
agent updates explicitly. Page namespaces are organizational, not access controls.

## Prepare each run

1. Choose a real task with a small, verifiable next result and one current owner.
2. Name clients A and B, their actor handles, the human evaluator, and the task ID.
3. Fix a time limit and write the expected evidence and completion criteria before
   the handoff. Keep the evaluator's answer key out of the checkpoint.
4. Record application/source revision, models, client versions, and environment.
5. Give A a page such as `tasks/pilot-01/checkpoint` and relevant source access.
6. A writes the checkpoint below, reads it back, and stops. Record the start state.
7. Start B in a fresh session with only the goal, checkpoint location, tool access,
   and normal project instructions. Do not pass A's conversation or hidden memory.
8. B reads the checkpoint and cited evidence, states its next action, and proceeds.
   The evaluator records every extra explanation or intervention the human gives.

Keep source references usable by both clients: page paths, repository revision and
file path, issue/document URLs, or artifact IDs. Mark inaccessible evidence rather
than claiming it was verified. Record timestamps with a timezone.

## Reusable checkpoint

```markdown
# <Task ID>: <short title>
Updated: <timestamp> | Owner: <actor handle> | Status: <in progress/blocked/done>

## Goal and completion check
<Desired result, scope, and how another person can verify completion.>

## Current state and evidence
- <Observed result> - <source reference and revision/date>, checked <timestamp>.
- <Assumption or unverified claim, clearly labeled>, attributed to <actor>.

## Decisions and discarded approaches
- <Decision, who made it, why, and evidence supporting it.>
- <Attempt that failed or was rejected, observed outcome, and when to reconsider.>

## Corrections and disagreements
- <Old claim> superseded by <correction>, source <reference>, changed <timestamp>.
- <A claims X. B claims Y. Evidence for each. Resolution still required.>

## Open questions
- <Question, missing evidence, and who or what can resolve it.>

## Environment and next action
Environment: <repository/version, relevant paths, tools, and verified constraints>.
Next action: <one concrete step, prerequisites, expected result, and verification>.
Refresh first: <checkpoint and source pages to re-read before continuing>.
```

## Five scenarios

| Run | Real-work setup | Required observation |
| --- | --- | --- |
| 1. Fresh-session handoff | Stop A during a bounded investigation or implementation. B resumes in a fresh session, preferably in a different client. | B identifies the actual state and completes the next step without rediscovering completed work or requesting essential background. |
| 2. Human correction | Use an actual mistaken assumption. After B's first read, pause its work. The human corrects the checkpoint and its supporting source. Tell B only that the checkpoint changed. | B manually refreshes, identifies the changed premise, and uses it in the next dependent action. Record the planned notification separately from unplanned hints. |
| 3. Discarded approach | Choose work where A already tried an approach that failed for a documented reason. Preserve the result and the conditions under which it might become viable. | B avoids repeating the same failed experiment, or explains new evidence that justifies retrying it. |
| 4. Conflicting claims | Select a genuine unresolved disagreement between participants or documents. Include attribution, dates, and both sets of evidence. | B preserves the disagreement and resolves it from evidence or explicitly abstains. It does not silently treat the latest statement as settled truth. |
| 5. Environment change | Resume a real task after a documented branch, tool, configuration, path, or service change. Preserve the earlier environment in the checkpoint. | B checks current conditions, spots the stale assumption, and adapts the next step instead of blindly replaying an obsolete procedure. |

Refresh the checkpoint and affected sources before each dependent action and
before writing an update. If another participant needs to edit, transfer writer
ownership first. Record this coordination cost as part of Stage 1's behavior.

## Minimal current API use

These shell examples assume `WIKIKIKI_BASE` and the current actor's
`WIKIKIKI_TOKEN` are already configured. Each actor uses its own token.
Keep tokens out of checkpoint bodies and result records.

```sh
# Read raw Markdown. Save elsewhere if checkpoint.md contains unsaved edits.
curl --fail --silent --show-error \
  -H "Authorization: Bearer $WIKIKIKI_TOKEN" -H "Accept: text/markdown" \
  "$WIKIKIKI_BASE/api/pages/tasks/pilot-01/checkpoint" -o checkpoint.md

# Create or replace the entire page from a UTF-8 file.
curl --fail --silent --show-error -X PUT \
  -H "Authorization: Bearer $WIKIKIKI_TOKEN" \
  -H "Content-Type: text/markdown" -H "X-Layer: semantic" \
  -H "X-Summary: Update pilot checkpoint" \
  --data-binary @checkpoint.md \
  "$WIKIKIKI_BASE/api/pages/tasks/pilot-01/checkpoint"
```

GET returns `text/markdown` unless Accept contains `application/json`. A missing
page returns 404. PUT requires authentication and returns JSON with `ok`, `path`,
`title`, and `layer`. These calls provide no revision token or conditional write.
Read back a successful write. Investigate an error before retrying: a failed
request may already have changed content, and retries have no idempotency key.

## Evaluation and plain-Markdown control

For each scenario, run a comparable fresh-session trial using the same checkpoint
template as a plain shared Markdown note with ordinary file tools. Give the control
the same evidence, source access, human update notifications, and writer schedule.
Use independent sessions and comparable task variants. Alternate which condition
runs first so the second agent does not inherit the first solution. Keep model,
time budget, and task difficulty matched. Record unavoidable differences.

Score each dimension 0 (wrong/missing), 1 (partial or needs an unplanned hint), or
2 (complete without an unplanned hint): next-step outcome, evidence accuracy,
current-state understanding, and scenario-specific behavior from the table. Record scores separately, with
supporting artifacts. Planned scenario notifications do not reduce these scores.
Record their coordination cost separately from unplanned restatement or hints.
Also record repeated work, elapsed time, context tokens and cost when available.
Use `unknown` for unavailable measurements.

A run passes only when its predefined completion check and scenario behavior both
succeed. Report any unplanned essential restatement separately. Five pairs are
diagnostic, not a statistically reliable performance claim. Preserve failures as well as wins.
Keep tuning examples separate by task/history family from future evaluation tasks.
After the pilot, select the smallest missing capability supported by repeated
failures, and rerun comparable held-out tasks before claiming an improvement.
