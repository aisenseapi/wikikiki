# Development roadmap

The product priority is simple self-hosting and a useful shared workspace for
people and their existing agents. AI SENSE publishes the software and explains
how to operate it. Running customer instances is outside this delivery model.

The wider architecture in DESIGN.md contains proposals. It is not a list of
features available today. This roadmap orders work by evidence from real use.

The [0 to 1 strategy](docs/ZERO_TO_ONE.md) defines the next product hypothesis:
make the consequences of a changed premise visible across ongoing agent work.
It includes prior work, a bounded experiment and criteria for rejecting the
direction. Its proposed capabilities are not part of the current application.

## Current foundation

The application has a browser editor, named actors, token and session login,
Markdown pages, Git history, revision checks, FTS search and task templates.
The current improvement batch adds recoverable page writes, portable path
validation, transactional credential administration, private defaults,
self-hosting packaging and conditional sync clients. It also records every
authenticated page read with its revision and shows, per changed page, who
read an earlier version and what they wrote before reading the current one.
That list is the first observed-read slice of the 0 to 1 experiment, with
precision and recall still to be measured.

The regression suite and local verification establish specific behavior.
An external deployment, independent security review and successful pilot are
separate evidence. Do not describe this as production maturity or state of the
art on the strength of unit tests.

## 1. Complete the external installation rehearsal

An operator who did not build Wikikiki must be able to:

1. Install from a tagged checkout, start a private instance and create an admin.
2. Connect one human and two separately identified agents.
3. Back up the instance, restore it elsewhere and verify pages, search, history
   and credentials.
4. Upgrade after taking a backup and recover using that backup if needed.
5. Remove a participant's access without deleting their attributed work.

Record the environment, source revision, elapsed time, failures and every place
where the operator needed help. Test Linux container and source installations,
then the documented Windows source path. Publish no supported-platform claim
without a passing installation and restore rehearsal on that platform.

A release needs passing format, lint, Rust and sync-client tests. Publish tagged
binaries or images only after the corresponding packaging is tested. Signatures,
checksums and a clear upgrade note belong with the first distributable release.

## 2. Prove the shared workspace is useful

Run [the handoff pilot](docs/HANDOFF_PILOT.md) with external self-hosters. Compare
against a plain shared Markdown note and an existing wiki or memory tool that
fits the same task. Record repeated explanations, stale claims, lost drafts,
wrong attribution, coordination effort and next-step success.

Start with the existing templates. Add activity navigation, a better comparison
view or explicit task state only when repeated failures identify the need.
Keep source references and uncertainty visible. A correction must be easy to
find and use at the next handoff.

The release gate is successful installation and task completion without ongoing
help from the developer. Five paired tasks diagnose problems. They do not
support a general performance claim.

## 3. Integrate the clients users actually run

Keep one write service and the same revision checks for every transport.
A thin MCP interface is a candidate once the pilot identifies required clients.
Test actual client versions and the complete read/update/conflict flow.

Possible additions include change cursors, events and client acknowledgments.
Document what they guarantee. A receipt that a client read a revision does not
prove that its subsequent reasoning or external actions used that revision.
Client idempotency keys and durable conflict drafts should precede unattended
retry-heavy integrations.

## 4. Improve retrieval and correction handling when measured

Basic Memory, Hindsight, Graphiti and established wikis already cover significant
parts of this space. Shared Markdown or correction propagation is not a novelty
claim. The opportunity is a coherent workflow with less installation and
coordination effort.

Evaluate each addition on held-out task families, against the strongest simple
baseline at comparable cost and latency:

- Revision-aware context packages with source links.
- Explicit dependencies so a correction identifies recorded downstream claims.
- Hybrid retrieval when FTS misses the information users need.
- Derived summaries that retain exceptions, uncertainty and source revisions.

Treat automated summaries and graph projections as rebuildable views. Never
promote retrieved page text into system instructions. Do not widen access through
a derived summary. Per-page permissions require enforcement across every read,
search, history and export surface before restricted content is introduced.

## Scope decisions

Keep Rust, SQLite, Markdown, Git and server-rendered HTML. Measure workload and
contention before adding another database, queue, vector service or distributed
writer model. Live working memory, autonomous distillation and federation remain
experiments until a concrete user problem warrants their operating cost.

wikikiki.com can host documentation, installation guidance, demonstrations and
release information. Owning the domain does not imply those services have been
published. Publishing or changing infrastructure is a separate action.
