# Wikikiki development roadmap

Date: 2026-09-19

Status: Proposed delivery plan with an implementation checkpoint below. This document proposes changes to the sequencing in DESIGN.md and does not silently replace its contracts.

## Implementation checkpoint, 2026-09-20

Stage 1 is being strengthened before wider integration. Current working-tree
changes include immutable Git write payloads, transactional metadata/edit/FTS
updates, escaped search snippets, token verification improvements, and the revised
interface. Page writes now retain one writer session through the database commit,
continue after caller cancellation, and validate layer and actor identity before
changing content. Normal shutdown drains accepted writes.

Validation on 2026-09-20: `cargo test --locked --offline` passed 50 tests
(37 unit tests, 6 existing HTTP integration tests, and 7 new page-consistency
tests). The new suite was also run against an isolated copy of the pre-change
source: 6 of its 7 cases failed there. Coverage includes 16 concurrent writers,
all persisted representations and actor attribution, rejected layers/identities,
and caller cancellation after an observed Git commit. This is regression evidence,
not a crash-recovery or multi-process guarantee.

These changes complete part of M1, not all of it. Cross-storage crash recovery,
revision checks, idempotent retries, restore verification, and existing dotted-path
compatibility remain open. Reads during a save can still observe different file
and metadata revisions. The coordination guarantee covers one shared `Repo`
instance, not multiple independently opened handles or server processes.

The initial Stage 2 drafts have been parked. The next product validation is
[five real handoffs](docs/HANDOFF_PILOT.md) using existing pages and explicit
client refresh. Complete missing capabilities in response to observed failures.
The larger milestones below remain a direction rather than a requirement to
implement the entire architecture before trying the product.

## Product direction

Build shared memory for the owner's agents and human collaborators first. The primary outcome is continuity across people, models, sessions, and machines.

A new participant should be able to determine what is happening, what evidence supports the current understanding, what remains disputed, and how to continue. A correction should reach the memories and tasks that depend on it.

The distinguishing product bet is inspectable, correctable collective memory. Git history, shared documents, semantic search, and automatic summaries are useful foundations but already exist elsewhere. Novelty and state-of-the-art performance are hypotheses to test, not claims this roadmap establishes.

The first deployment is one controlled internal instance serving a small set of real projects and at least two different agent clients. Wikikiki owns memory, provenance, and coordination state. Existing runtimes continue to execute agents and external actions.

## Starting point

The source repository root is this directory, locally `code/rust`. Files here appear at the root of `aisenseapi/wikikiki` when pushed. The adjacent PHP skeleton is outside this repository.

The inspected baseline is commit `bf7a13f`. It implements Rust/Axum, server-rendered Maud HTML, Comrak, SQLite/FTS5, Markdown files, Git through `gix`, cookie sessions, bearer tokens, and actor attribution. Six HTTP integration tests cover basic access and page flows. This planning review inspected source and tests but did not execute them.

During the initial review, untracked Stage 2 drafts appeared in `migrations/20260919120005_threads.sql` and `src/events.rs`. They were subsequently parked outside the active source tree. They were not assessed as an integrated release. Reconcile future implementation with the current repository and any explicitly restored drafts before assigning overlapping work.

The three memory layers remain the organizing model:

| Layer | Purpose | Durable representation |
| --- | --- | --- |
| L1: working memory | Current task state, open questions, checkpoints, participants | SQLite current state plus append-only changes |
| L2: episodic memory | What happened, which decisions were made, and their outcomes | Markdown and Git, linked to source events |
| L3: semantic memory | Reusable knowledge and procedures with supporting evidence | Markdown and Git, with structured provenance and dependencies |

Layers do not measure truth. A semantic page can contain an unresolved hypothesis. Separate memory lifetime from evidence status.

## What the current landscape changes

Research and product documentation were checked on 2026-09-19. These are selected primary sources, not an exhaustive novelty review. Reported benchmark results are the authors' results under their configurations.

| Source | Relevant evidence | Planning consequence |
| --- | --- | --- |
| [Letta Context Repositories](https://www.letta.com/blog/context-repositories/), February 2026 | Git-backed file memory, progressive disclosure, reflection, and concurrent memory work through worktrees are documented features. | Git-backed Markdown and background reflection alone will not differentiate Wikikiki. |
| [Graphiti](https://github.com/getzep/graphiti) | Temporal facts, provenance, incremental updates, and hybrid retrieval are documented capabilities. | Time and source identity belong in the model. A graph database is an implementation choice to evaluate. |
| [Hindsight](https://aclanthology.org/2026.acl-demo.27/), ACL July 2026 | Separates facts and beliefs and combines temporal, graph, keyword, and vector retrieval with reflection. | Treat these as comparison capabilities, not unique inventions. |
| [GroupMemBench](https://arxiv.org/abs/2605.14498), May 2026 | Tests multi-party memory, speaker attribution, updates, ambiguity, and abstention. Its strongest evaluated system reaches 46.0% average accuracy. | Preserve who said what and test group corrections. Keep a strong lexical baseline. |
| [GateMem](https://arxiv.org/abs/2606.18829), June 2026 | Tests shared-memory usefulness, contextual access, and forgetting together. The evaluated systems show tradeoffs across these requirements. | Permission and withdrawal behavior must be tested across derived memory as well as original records. |
| [LongMemEval-V2](https://arxiv.org/abs/2605.12493), May 2026, work in progress | Evaluates workflow knowledge, changing state, environment pitfalls, and false premises, with explicit accuracy/latency tradeoffs. | Measure whether memory helps a colleague continue work, not only answer recall questions. |
| [MCP 2026-07-28 release](https://blog.modelcontextprotocol.io/posts/2026-07-28/) | Revises transport, subscriptions, discovery, and authorization. The release describes Rust support as beta at publication. | Verify current Rust SDK and actual client compatibility before choosing protocol versions. Browser SSE and MCP transport are separate decisions. |

The proposed opportunity is the combination of reliable collaboration, source-linked corrections, useful human inspection, and measured task continuity. Comparable capabilities may exist elsewhere. Maintain an explicit comparison as the prototype develops.

## The first convincing demonstration

Use one real project and one bounded investigation involving two agent clients and a human.

1. Agent A records the goal, evidence, attempted approaches, unresolved questions, and next action.
2. Agent A stops. Agent B starts with a fresh session and receives a compact context package with source links and revisions.
3. Agent B continues successfully without the human restating the project.
4. The human corrects an important assumption while Agent B is working.
5. Wikikiki records the correction, identifies recorded dependencies, marks affected conclusions for review, and delivers a change notification. The client acknowledges the new revision before its next dependent write.
6. The episode becomes a useful permanent note. A later comparable task avoids the original mistake.

This is the recurring demonstration for the roadmap. Do not substitute a visually impressive activity feed for evidence that the handoff and correction worked.

External clients can retain context they have already received. Wikikiki can enforce freshness and access at its own interfaces, while client adapters must implement refresh and acknowledgment. It cannot guarantee that arbitrary external actions use fresh knowledge.

## Delivery sequence

The order below is intentional. Evaluation starts immediately and continues throughout. M3 research can begin alongside M2, but depends on M2's identity and revision contracts before release.

| Milestone | User-visible result | Dependency | Completion evidence |
| --- | --- | --- | --- |
| M0: baseline | A repeatable example of today's handoff failures and costs | Existing Stage 1 | Versioned scenarios, runnable baseline, deployment inventory |
| M1: reliable memory | Writes retain the correct content, actor, and history through concurrency and restart | M0 | Concurrency, retry, and crash-recovery tests |
| M2: shared work | Two clients and a human work in one live task and resume after interruption | M1 | First handoff demonstration, reconnect and access tests |
| M3: useful context | Each participant gets current, relevant evidence within a context budget | M2 | Improvement over lexical/file baselines on held-out tasks |
| M4: correction and distillation | Corrections expose affected knowledge and summaries preserve important exceptions | M2 and M3 | Dependency, omission, and stale-knowledge evaluations |
| M5: learning from outcomes | Successful methods are reused and repeated mistakes become less frequent | M4 | Repeated-task gains and controlled ablations |

### M0: establish the baseline

- Inventory the actual first two clients, host OS, deployment hardware, model endpoints, and acceptable data flows. Use the existing OpenClaw integration where it matches real usage.
- Review concurrent Stage 2 work against the current repository and map completed pieces to this roadmap before assigning implementation tasks.
- Run the existing Rust tests on a supported toolchain and record the result. Establish a reproducible local run and CI path.
- Create 20 initial scenarios from real work, including handoff, correction, concurrent changes, stale assumptions, disagreement, and access changes. Reserve held-out cases before tuning.
- Record the current Markdown/FTS5 workflow as a baseline. Include a file-search workflow and a long-context control where feasible.
- Measure task success, missing facts, human restatement, context tokens, latency, and operating cost. Separate ingestion and background cost from query cost.
- Write short architecture decisions for canonical page paths, revision checks, cross-storage recovery, and visibility defaults.

Planning allowance: approximately 2-3 focused engineering days, subject to toolchain and deployment findings. Re-estimate later work from this baseline instead of committing to a calendar for the whole roadmap.

### M1: make the shared memory dependable

Inspection of the original baseline found prerequisites that should move ahead of the original Stage 4 hardening work. Some are now partially addressed, as recorded in the dated implementation checkpoint above:

| Observed implementation | Consequence | First change |
| --- | --- | --- |
| `src/pages/mod.rs` writes the file before `src/git/mod.rs` acquires its lock and rereads the file | Concurrent requests can attribute another request's bytes to the wrong actor | Carry immutable content through one coordinated write operation |
| Page metadata, edit history, and FTS updates use separate database operations after Git commits | A partial failure can leave different views of the same page | Transactional database projections plus a durable operation journal and recovery |
| Page validation permits dots while `file_path` replaces the extension | Different page identifiers can address the same file | Define one canonical mapping and a migration/check for existing aliases |
| Writes have no expected revision or idempotency key | Stale clients overwrite changes and retries duplicate history | Versioned writes with explicit conflict and retry behavior |
| Reindex/reconciliation is described in DESIGN.md but absent from the CLI/server | Recovery currently depends on manual intervention | Implement inspect, repair/reindex, backup, and restore commands |

These are static source findings. Reproduce the failure modes with regression tests before changing the implementation.

Build one application service used by UI, REST, and later MCP. A write carries an authenticated actor, canonical resource ID, immutable payload, expected revision, and operation ID. Validate authorization, paths, and referenced vocabulary before side effects. Scope idempotency to actor and operation, and reject key reuse with different content.

For L2/L3, retain committed Git content as the durable content authority. Use a durable journal to associate operation IDs, payload hashes, revisions, and commits. SQLite metadata and search projections advance transactionally. Define exactly when a write is acknowledged and how restart completes or reconciles each interrupted step. A SQLite transaction cannot atomically commit Git and files. The journal protocol must address that boundary explicitly, and must retain provenance needed to rebuild metadata rather than relying on commit author strings alone.

Start with one coordinated writer per content repository and bounded backpressure. Define a single-server process ownership rule. An in-process mutex does not coordinate multiple servers or out-of-band editors. Import external file changes through a reconciler with explicit attribution.

Use revision checks to detect stale writes and preserve submitted alternatives. Store a conflicting submission durably with actor, base revision, operation ID, and a retrievable conflict ID, without changing the current main version. Recovery must distinguish a preserved conflict from an interrupted operation that should complete. A rejected stale update is a consistency response, not a human approval gate. Automatic retry or merge is allowed only when its preconditions remain valid.

Completion requires:

- Deterministic concurrent-write tests prove matching content, hash, actor, revision, Git blob, edit record, and search result.
- Failure injection at each persistence boundary followed by restart yields a documented recoverable state.
- Retrying a completed operation produces the original outcome once.
- Canonical path tests cover extension aliases and existing-content migration.
- A backup can be restored into a fresh instance and checked for consistency.
- Required local-state ignore rules are completed without modifying or committing native history.

Initial planning allowance after M0: 1-2 engineering weeks. The reproduction and recovery design may change this estimate.

### M2: put two agents and a human in the same work

Implement task threads, participants, transitions, current working memory, append-only memory edits, and durable events. Each L1 update and its audit/event records commit in one SQLite transaction. Notifications are delivered from durable events, with cursors, replay, deduplication, and bounded retention. An expired cursor returns an explicit resynchronization response requiring a consistent snapshot and a new cursor. Test the snapshot-to-stream boundary for missing updates.

Add a small task-oriented interface showing the goal, current state, next actions, sources, disagreements, and changes since the viewer last visited. Provide actor and revision links at the point of use. Browser updates use SSE with reconnect support. Presence is advisory and expires after a heartbeat timeout.

Expose the same service through REST and MCP. Begin with page read/write/search, thread open/read, working-memory get/set, event append, and changes-since operations. Keep domain thread IDs and cursors explicit and independent of transport sessions. Pin and test protocol versions against the selected clients. Resource subscriptions are optional client capabilities, with polling as a fallback.

Resolve the contradictory visibility defaults in DESIGN.md before implementation. Proposed initial behavior is explicit scope at thread creation, with shared scope for normal team work. Restricted scopes ship only when their policy is enforced for direct reads, search, history, exports, event replay, and derived content. Recheck current permission during streaming and replay.

Ship a thin client contract: read current task state on entry, retain a revision cursor, refresh relevant changes before dependent writes, and record a checkpoint before stopping. At M2 this is a client refresh contract. M1's destination revision check alone cannot establish that every source informing a write is current. Store task state, evidence, decisions, outcomes, and brief rationales. Do not require private model reasoning or complete conversation dumps as the collaboration format.

Completion requires a fresh session in a different client to continue the demonstration task. Both clients must observe a human correction and recover after disconnect. Permission removal must prevent subsequent server retrieval and event delivery. Existing context copies remain a documented client boundary.

Initial planning allowance after M1: 2-3 engineering weeks, depending on client and SDK compatibility.

### M3: return useful context, with evidence

Introduce a context service that accepts task, actor, scope, query, revision cursor, and token budget. Return a small structured package containing current task state, relevant knowledge, new changes, unresolved contradictions, sources, and resource revisions. Explain inclusion briefly and allow a client to retrieve original evidence.

Allow dependent writes to declare the source IDs and revisions they used. Validate those declared dependencies as part of the write protocol, separately from the destination's expected revision. This establishes freshness for the declared read set, not for unreported knowledge inside an external client. M4 extends these records into correction impact analysis.

Improve retrieval incrementally: FTS5 with actor/time/task filters, then optional embeddings and fused ranking, followed by reranking only if evaluation justifies its latency and cost. Filter access before retrieval output, snippets, ranking explanations, and context assembly. Keep embeddings rebuildable and version their model/configuration.

Preserve time explicitly: when an event happened, when it was recorded, and, where known, when a claim applies. Support both current-state and historical questions. Do not infer that the most recently written claim is correct.

Separate assertion type from status. Initial types can cover observations, hypotheses, decisions, and procedures. Status can distinguish active, disputed, superseded, and needing review. Store author, subject, supporting source revisions, and scope. Source evidence and observed outcomes determine status changes. A model's confidence number is not a substitute for evidence.

Completion requires an improvement over the fixed baseline on held-out tasks at a declared cost/latency budget, including correct abstention and preservation of disagreement. Keep lexical search available even if a semantic component wins overall.

### M4: make corrections and distillation first-class

Maintain explicit dependency records between source revisions, assertions, summaries, procedures, and task context packages. On correction or withdrawal, find recorded descendants, mark them for review, suppress stale conclusions from normal current-state retrieval, and notify affected active tasks. Re-evaluation can confirm or replace the conclusion. It must not blindly invert every descendant.

Start dependency storage in SQLite with typed links. Human inspection should answer: "What supports this?", "What changed?", and "What else depends on it?" Support historical reconstruction of memory state and decisions, without replaying external tool actions.

Distillation runs asynchronously with a durable queue, bounded concurrency, retry control, and a resource budget. Preserve source references, source revision/hash, kept and omitted material, rationale, output revision, actor, and model/policy version. The first automation should handle one bounded task-completion summary before attempting continuous global rewriting.

Apply incremental changes to L2/L3. Keep counterexamples, exceptions, and contradictory evidence visible. Execute against an explicit source snapshot, check whether the source changed before publishing, and retain the previous output. The normal workflow can remain automatic and observable, with targeted human inspection of contested or uncertain results.

Add retention and withdrawal semantics across sources, projections, derived summaries, caches, and embeddings. Distinguish removal from active context from physical deletion of retained history. Git history and backups prevent a casual promise of complete erasure. If physical deletion is needed, define it as a separate operator workflow with a clear retention contract.

Completion requires a known source correction to identify every explicitly recorded dependent item in a deterministic fixture. Evaluate semantic dependency discovery separately for precision and recall. Test omitted exceptions, stale summaries, and the propagation of access restrictions. Never widen the source audience automatically through a summary.

### M5: test the more ambitious ideas

Run these as bounded experiments after the core is used daily. A feature graduates when it improves held-out task outcomes enough to justify its complexity and cost.

| Experiment | Hypothesis | Evaluation |
| --- | --- | --- |
| Context tailored to the next action | A small, revision-aware context package beats a generic summary | Paired task success at equal context and latency budgets |
| Correction impact analysis | Dependency-aware refresh reduces reuse of invalid assumptions | Stale-claim use and correction latency with the feature on/off |
| Procedures learned from outcomes | Versioned methods linked to successes and failures reduce repeated mistakes | Held-out related tasks, with environment/version changes included |
| Competing interpretations | Keeping alternative hypotheses separate improves difficult investigations | Final correctness, premature consensus, and unnecessary review work |
| Memory policy comparison | Alternative distillation/retrieval policies can be tested on the same history | Offline replay of memory transformations with fixed inputs and no external actions |

Start with correction impact analysis. It directly supports the product promise and builds on information the system already needs to retain.

## Evaluation and release criteria

Build a small internal suite from the owner's work, then expand from 20 development scenarios to at least 50 held-out scenarios as real usage supplies examples. Split development and evaluation by task/history family, not just by individual question. Freeze the held-out set before tuning. Evaluate learned procedures on new comparable tasks, not reworded questions about the same answers. Public benchmarks provide additional comparisons. They do not replace multi-client handoff tests.

| Capability | Measure |
| --- | --- |
| Handoff | Task completion after a new actor/session takes over, plus human restatement required |
| Current knowledge | Correct use of updated facts and rejection of obsolete premises |
| Attribution | Correct separation of actor statements, observations, and agreed decisions |
| Evidence | Source accuracy and revision validity for retrieved assertions |
| Distillation | Retention of required details, exceptions, and disagreements |
| Collaboration | Lost updates, duplicate operations, and event replay gaps |
| Boundaries | Unauthorized or withdrawn content appearing in reads, search, streams, or derived context |
| Efficiency | Context tokens, ingestion/background/query cost, p50/p95 latency, storage growth |
| Operational recovery | Time and completeness of recovery after process crash and backup restoration |

Compare the same histories, task runner, model revision, prompts, and budgets where possible. Report unavoidable integration differences. Include no-memory, current FTS5, file search, and a selected external memory baseline. Pin versions and use multiple runs for model-dependent outcomes. Report uncertainty and per-category results, not just one average score.

Provisional internal prototype targets, to ratify after M0:

- At least 90% successful handoffs on the held-out internal suite, without the human restating essential context.
- At least 95% correct handling of corrections on the explicitly labeled correction subset.
- Zero lost acknowledged writes, wrong-actor content, or forbidden disclosures in the deterministic regression suite. This is a test criterion, not a universal production guarantee.
- A useful quality/cost improvement over the strongest simple baseline, with a predeclared latency budget on named deployment hardware.

Memory content is input data. Include cases where historical notes contain misleading instructions or fabricated authority. Retrieval must not promote such text into system policy or tool permissions. Evaluate recovery from contaminated memory alongside preservation of legitimate content.

## Architecture choices and scope

Keep Rust, SQLite, Markdown, Git, and server-rendered HTML for the first internal deployment. Add small interactive components where the live task view needs them. Measure SQLite contention and Git throughput under the actual workload before choosing another database or distributed design.

Keep operational memory repositories separate from the application source repository. Use consistent backup boundaries for content, database, and recovery journal. In particular, deploy the live SQLite database on local storage rather than relying on a cloud-synced development folder as the running data store.

Treat vector indexes and dependency graph views as rebuildable projections. A separate graph database, CRDT editor, additional federation layer, or hosted multi-tenant offering needs evidence from the internal prototype before becoming a delivery dependency.

Integrations should ingest only configured, authorized sources and retain origin IDs, scope, timestamps, and revisions. Native `.agent-state` files remain read-only. Any future import of selected historical material is explicit and produces derived records without rewriting native history. An import source is not automatically an instruction authority.

## Changes proposed to DESIGN.md

1. Move write integrity, recovery, revision checks, and baseline evaluation ahead of live multi-agent use.
2. Implement visibility enforcement with the first restricted resource rather than deferring it to final hardening.
3. Replace silent last-writer-wins as the default client contract with explicit revision checks and preserved alternatives.
4. Clarify that a Git commit and a SQLite transaction are separate durability boundaries with a recovery protocol.
5. Resolve the inconsistent thread visibility defaults and remove outdated implementation statements such as the README's `git2` reference.
6. Add assertion status, source revisions, temporal validity, dependency links, and correction propagation to the memory model.
7. Define portable task checkpoints and client refresh behavior instead of assuming every agent will externalize all cognition.
8. Refresh the MCP integration against supported client/protocol versions instead of copying the original transport assumptions.

These changes should be recorded in architecture decisions and reflected in DESIGN.md as their milestones are implemented.

## First implementation batch

The next implementation session should take only M0 and the first M1 slice:

1. Reproduce the concurrent content/actor mismatch and page-path aliasing.
2. Record the canonical path and versioned write contracts, including compatibility for the existing sync script.
3. Introduce the immutable write request and consistent actor/content handling.
4. Group SQLite projections in a transaction and specify the crash-recovery journal before extending the pipeline.
5. Add targeted regression tests and demonstrate two competing writes with preserved evidence.
6. Record baseline test and performance results, then refine estimates for the remaining M1 work.

This batch establishes the write contract and a measurable starting point. M2 release still depends on completing all M1 recovery, concurrency, and restore criteria.
