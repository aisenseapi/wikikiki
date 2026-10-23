# Wikikiki: a testable path from 0 to 1

Date: 2026-10-23

Status: product hypothesis and experiment plan. The proposed capabilities below
are not implemented unless explicitly listed as current behavior. No experiment
results or claims of unique capability are established by this document.

Audience: the product owner, contributors and people evaluating the direction.

Contents:

- [The product decision](#1-the-product-decision)
- [The problem and the first users](#2-the-problem-and-the-first-users)
- [What exists today](#3-what-exists-today)
- [Prior work and the novelty boundary](#4-prior-work-and-the-novelty-boundary)
- [The reference scenario](#5-the-reference-scenario)
- [Product behavior to test](#6-product-behavior-to-test)
- [Minimum experiment scope](#7-minimum-experiment-scope)
- [Proposed data and consistency model](#8-proposed-data-and-consistency-model)
- [Trust, privacy and self-hosting](#9-trust-privacy-and-self-hosting)
- [Experiment design](#10-experiment-design)
- [Continue, narrow or stop](#11-continue-narrow-or-stop)
- [Delivery sequence and review gates](#12-delivery-sequence-and-review-gates)
- [Product position if the evidence supports it](#13-product-position-if-the-evidence-supports-it)
- [Evidence and decision record](#14-evidence-and-decision-record)

## 1. The product decision

Wikikiki should give people a useful new ability when working with several
agents. Simple self-hosting and a shared workspace provide the foundation.
The next investment should test whether that foundation can support a distinct
workflow that existing tools handle poorly.

The leading hypothesis is:

> Change a project premise once. See which ongoing work it affects, help the
> responsible people and agents repair that work, and preserve the result for
> the next handoff.

The intended benefit is less repeated explanation and less work based on
superseded assumptions. A person should be able to inspect what changed,
understand the consequences and see evidence of follow-up.

For this project, 0 to 1 means a meaningful new capability for the intended
user. It does not require inventing every underlying technique. It does require
evidence that the resulting workflow offers more than a small convenience
around an existing wiki or memory system.

Technical originality, user value and a sustainable product advantage are
different questions. The experiment must examine all three without treating
one as proof of the others.

### Delivery model

AI SENSE publishes software and explains how other people can operate it on
their own machines or servers. Customer instances remain under their operators'
control. The direction does not depend on AI SENSE hosting customer data.

The repository is [aisenseapi/wikikiki](https://github.com/aisenseapi/wikikiki).
The owner also has wikikiki.com. The domain can support documentation, examples,
downloads and demonstrations. Ownership alone establishes no deployed service
or product advantage.

### Immediate priority

Keep the existing application stable and test the correction workflow before
expanding the feature set. External installation and restore rehearsals remain
release requirements. Product discovery can run locally in parallel with that
operational work.

## 2. The problem and the first users

A team can update a document while an agent continues from an earlier plan.
Another agent can inherit that plan without knowing which assumptions produced
it. The latest information may be available somewhere, yet the work still
reflects the old requirement.

The person coordinating the work then has to discover the mismatch, repeat the
correction, locate affected outputs and check whether each participant adapted.
Updating the source does not complete this coordination work.

The initial user hypothesis is a technical owner or small trusted team that:

- Uses at least two agent clients on the same project.
- Changes requirements or corrects assumptions during active work.
- Already keeps some shared documentation and wants to retain control of it.
- Can operate a small self-hosted service.
- Has experienced repeated explanations or contradictory outputs at handoff.

This is a proposed starting audience. Interviews and observed work must confirm
that the problem occurs often enough to justify another tool.

The first use case is software project coordination, where plans, documentation
and bounded repository artifacts are available for inspection. High-impact
external actions are outside the initial experiment.

Questions for early users should concern recent events:

- What was the last correction you had to repeat to another agent?
- Which outputs had already been produced from the old assumption?
- How did you discover the mismatch, and what did repairing it require?
- Which existing tool or habit already prevents the problem?
- Would recording dependencies create more work than the correction saves?

Positive reactions to an idea are weaker evidence than actual task outcomes and
voluntary repeat use.

## 3. What exists today

This assessment refers to the local working tree inspected on 2026-10-23.
It must not be read as a claim that every local change is already published on
GitHub or included in a release.

| Area | Current behavior | Relevance to the experiment |
| --- | --- | --- |
| Shared content | Markdown pages, browser editing and a REST API | People and agents can use the same recorded material |
| Identity | Named actors, browser sessions and individual tokens | Changes can be attributed to participants |
| Revisions | Opaque page revisions and conditional updates | Conflicting writes can be detected when clients use the conditions |
| History | Git history and historical page views | Earlier text and attributed edits can be inspected |
| Retrieval | SQLite FTS search | Provides a simple retrieval baseline |
| Handoffs | Templates for tasks, decisions and knowledge | Supports manually maintained continuity |
| Recovery | Pending-write journal and replay for interrupted page writes | Provides an existing storage discipline for future extensions |
| Reads | Each authenticated read records actor, served revision and surface | The observed-read evidence class exists without any client declaring anything |
| Affected work | Per changed page: readers of earlier revisions, whether they have read the current one, and pages they wrote in between | A first impact list from timing alone; precision and recall are unmeasured |
| Operations | Self-hosting instructions and packaging in the working tree | Supports operator-controlled experiments |

The current API's revision check protects the page being written. It does not
validate every source used to prepare that write. Reading a source again does
not by itself prove that the agent revised a plan derived from an older source.

Declared task dependencies, review outcomes, correction propagation, live
notifications and action validation are not current capabilities. The
affected-work list is inference from read and write times: it can name a page
that does not depend on the change and miss one that does. A recorded read
does not establish understanding or compliant future behavior.

The current instance model is one trusted group. Authenticated participants
share page access. Actor types and namespaces do not provide page-level access
control. The proposal must not imply stronger permissions.

Use [the README](../README.md), [the current design contract](../DESIGN.md),
[the agent API](AGENT_API.md) and [self-hosting guidance](SELF_HOSTING.md) for
implemented behavior. The wider design proposals remain proposals.

## 4. Prior work and the novelty boundary

The following primary sources were checked on 2026-10-23. This is a focused
comparison, not an exhaustive market survey or proof that no other equivalent
product exists. Published research results have not been reproduced here.

| Prior work | Relevant overlap | Consequence for Wikikiki |
| --- | --- | --- |
| [Basic Memory Teams](https://docs.basicmemory.com/teams/about) | Shared knowledge for people and connected agents. The documented team offering is subscription based, while local workspaces have no team features. | Shared memory across agent clients is already a product category. Deployment differences may matter to users, but do not establish a new capability. |
| [BookStack](https://www.bookstackapp.com/docs/) | Established wiki software with installation, administration and recovery documentation. | A usable self-hosted wiki is an existing baseline. |
| [Hindsight memory curation](https://hindsight.vectorize.io/developer/api/memories) | Corrections and invalidation of source facts can cause derived observations to be recomputed. | Propagating a correction through derived memory is already supported elsewhere. |
| [Graphiti](https://github.com/getzep/graphiti) | Temporal facts, source provenance and invalidation while retaining history. | A temporal knowledge graph or provenance graph is not sufficient differentiation. |
| [Planfence](https://arxiv.org/abs/2609.03340v2) | Links inherited plans to recorded inputs and checks their current versions before protected actions. Application code supplies the links and scope. | Checking whether a plan's premises changed has direct research precedent. |
| [ATR](https://arxiv.org/abs/2609.08015) | Revalidates explicit executable conditions for pending actions. Distinguishes changed versions from changes that invalidate a decision. | Selective decision validation and avoiding unnecessary replanning are established research directions. Its controlled results do not establish automatic extraction of conditions. |
| [Concord](https://arxiv.org/abs/2610.05281v2) | Links tool observations to mutable sources and updates, annotates or suppresses stale context. | Keeping an agent's active context aligned with source changes also has close prior work. |
| [Dependency-guided rollback repair](https://arxiv.org/abs/2608.10502v2) | Traces faulty memory into affected computation and selectively repairs persistent state while retaining unaffected work. | Repair after a memory error has direct precedent. Selective repair also has measured tradeoffs in the paper's controlled setting. |

The potential opening is a coherent human workflow: a correction produces an
understandable account of affected work, attributable follow-up across clients
and a usable handoff, with little manual maintenance. That opening remains an
inference and must be tested against existing products and simple adapters.

The difficult research question is whether useful dependencies and consequences
can be captured with sufficiently low effort and sufficiently few mistakes.
Version tracking alone cannot answer it. An agent-generated dependency list may
omit exactly the premise that matters.

Before adopting code from another project, inspect its license and integration
cost. A citation is not a license to incorporate an implementation.

## 5. The reference scenario

Use a real project decision with inspectable consequences:

> Customers will run this software themselves. The design must not require a
> service operated by the publisher.

Before that decision is clarified, agent A has drafted an installation guide
and agent B has drafted an architecture with a publisher-operated dependency.
A separate task is correcting the application's keyboard navigation.

The proposed workflow is:

1. The human updates the project premise and records why it changed.
2. Wikikiki identifies work with recorded dependencies on the earlier premise.
   It can also suggest other possibly affected work, labeled as inference.
3. The workspace shows the old and new premise, the relevant part of each
   artifact, the reason for the relationship and the responsible participant.
4. The installation task is reviewed against the new requirement. If it already
   supports independent operation, that conclusion is recorded with evidence.
5. The architecture task is revised to remove the required publisher-operated
   service. The old artifact remains available in history.
6. Keyboard navigation work continues unless a specific relevant relationship
   is found. A shared project name alone does not establish a dependency.
7. A fresh agent session sees which work was repaired, what remains unresolved
   and which source revisions support the next step.

The desired outcome is that the human explains the requirement once and can
inspect its consequences without reading every agent conversation.

A changed requirement does not automatically make every old output wrong.
The workflow must support an evidence-backed finding that existing work remains
valid. Disagreement about the requirement remains visible until an appropriate
participant resolves it.

## 6. Product behavior to test

### The human view

Start with an ordinary list of affected work. Each entry should answer:

- What changed and where can I inspect the source?
- Which task, plan or artifact might be affected?
- Is the relationship recorded or inferred?
- Why does the change matter for this item?
- Who is following up and what evidence closes the issue?

Useful actions include opening the source comparison, assigning follow-up,
recording that work remains valid, linking a replacement artifact and disputing
an inferred relationship. A graph visualization is optional and must justify
its complexity through actual use.

The interface should show unresolved work without implying that a person must
approve every ordinary agent action. Existing attribution and correction in
flow remain useful defaults.

### The agent view

A participating client needs a compact account of changes relevant to its
registered work, with source revisions, evidence and unresolved questions.
It should be able to record a repair or explain why no repair is needed.

Reading, acknowledging, evaluating and repairing are separate events. The
interface must not collapse them into a single claim that an agent understood
the change. An abandoned task must remain visible rather than silently becoming
complete after a timeout.

### Optional enforcement

A later integration may check registered dependencies before a specific action.
This is an extension to the current design, which has no general approval queue.
It requires a separate, explicit execution contract.

The first experiment is advisory. A simulated action may demonstrate validation
without controlling deployments, messages, purchases or other external effects.
Product value must be measured independently of how many actions are blocked.

## 7. Minimum experiment scope

Build the smallest vertical slice that can expose a failure of the hypothesis:

- One local instance, one human and two distinct agent clients.
- A small set of project premise pages and registered work items.
- Revision-bound relationships between those sources and work items.
- A comparison and affected-work list in the browser.
- Attributed review outcomes and a fresh-session handoff.
- Instrumentation for manual effort, repair outcomes and false alarms.

Start with explicit relationships to test storage, propagation and the user
workflow. Then run a separate assisted-capture condition where clients suggest
relationships from their actual source reads and work artifacts. Score its
misses and the effort required to correct them.

The explicit condition establishes mechanical feasibility. It cannot prove
that the user avoids maintaining a dependency graph by hand.

Exclude federation, autonomous long-term distillation, broad connector catalogs,
new databases and a generic agent orchestration platform from this experiment.
Add retrieval techniques only when observed misses justify them. Keep the
existing wiki usable even if the experiment is disabled or rejected.

## 8. Proposed data and consistency model

This section describes design requirements for an experiment. It is not a
database migration, a supported API or a promise of exact field names.

### Records and their meaning

| Record | Minimum information | Interpretation |
| --- | --- | --- |
| Source reference | Resource identity, opaque revision, optional passage anchor and captured excerpt | Identifies the evidence that was actually available |
| Work item | Stable ID, owner, goal, artifact reference and progress state | Identifies work whose basis may need review |
| Dependency | Source reference, work revision, relationship type, reason, recorder and capture method | States a recorded relationship, with its provenance |
| Change | Previous and new source revision, editor and explanation when supplied | Records a change without declaring its downstream meaning |
| Impact finding | Change, affected work revision, dependency path and evidence class | Makes a possible consequence inspectable |
| Review outcome | Finding, actor, evaluated source and work revisions, outcome and evidence | Records a bounded conclusion that can itself become stale |

Separate dependency evidence into observed reads, agent suggestions and explicit
declarations that work relies on a premise. Reading a page is a candidate signal.
It does not prove every part of the page influenced the result.

Observed reads are the one class the server can collect without client
cooperation, because every client reads through it. That is also the cheapest
answer to the capture-cost question in section 4, and it is implemented: the
affected-work view lists, for a changed page, who read an older revision and
what they wrote before reading the current one. Its false-positive and miss
rates are unmeasured and belong in the first development cases.

Keep task progress separate from review status. A completed task can need review
after its premise changes. A task in progress can remain unaffected.

Suggested review outcomes are `repaired`, `still_valid`, `disputed` and
`unresolved`. Each outcome must identify the revisions evaluated. None proves
that all relevant dependencies were captured.

### Deterministic boundary

For registered dependencies, a source revision mismatch must be observable.
A mismatch requires reconciliation before claiming current status. This may be
automatic when the evidence permits it. Semantic invalidity requires additional
evidence. Metadata-only page edits also change current Wikikiki revisions, so the
simplest detector will intentionally flag some harmless changes.

For a simulated protected action, the narrow invariant is:

> At validation time, an action must not receive a current-status result when
> a dependency in its declared scope has changed and has no review outcome
> covering the exact source and work revisions being validated.

The result says nothing about missing dependencies or tools outside the
integration. A newer page version can contain the same relevant meaning. A
review can establish that the work remains valid without pretending the revision
never changed.

### Concurrent changes and recovery

Validation must read a coherent set of published source heads and work records.
If a model evaluates meaning outside the write transaction, accepting its result
must recheck the exact source, work and dependency revisions it evaluated.
A changed input makes that result stale and requires another evaluation.

The current per-page conditional write is insufficient for this multi-resource
operation. Prototype persistence must use an explicit transaction or equivalent
generation check. Do not describe several independent GET requests as an atomic
snapshot.

Durably record any event needed to reconstruct impact findings at the same
publication boundary as the relevant state change, or use a recoverable outbox.
Consumers must tolerate duplicates, retries and reordered delivery. Missing
events after restart must be detectable by reconciling stored source revisions.
These are requirements for new code, not guarantees of the existing page API.

No model or network request should hold the repository writer lock. Preserve the
existing page-write journal and add meaningful fault and concurrency checks when
extending its publication boundary.

Bound traversal of indirect dependencies and detect cycles. If a bound is
reached, report incomplete analysis. A missing source, broken passage anchor or
inaccessible artifact is an unknown result rather than evidence of validity.

### External actions

A check can become stale between validation and an external effect. Closing that
gap requires cooperation from the target, such as a transaction or a conditional
operation tied to the checked state. A gateway response alone cannot guarantee
that the target executed against the same state.

An outcome receipt must distinguish requested, accepted, executed and unknown.
Retry safety requires idempotency support or reconciliation at the target.
Revising a plan cannot undo an email already sent or another irreversible effect.

## 9. Trust, privacy and self-hosting

Stored pages, imported text and agent history remain untrusted content. A page
claiming that an action is permitted cannot grant credentials or override an
operator's configured policy. Dependency tracking must preserve this boundary.

Do not import complete native agent histories by default. Use bounded source
references and intentional artifacts. AgentSync history remains read-only
historical context and is not the canonical store for new product state.

The prototype inherits the trusted-group access model. It cannot make private
task or team claims while all authenticated actors share access. If narrower
permissions are introduced later, enforce them consistently on dependency
paths, findings, excerpts, search, history, notifications and exports.

Keep revision tracking and recorded relationships usable without a model
provider. Assisted semantic analysis may use a model chosen by the operator.
Document exactly which excerpts leave the instance, how credentials are kept
and what happens when the provider is unavailable. Self-hosting the database
does not imply local inference when a remote model is configured.

The service should degrade to manual review with visible unknown results.
Record analysis latency and cost so that an optional model does not quietly
become an operational requirement for ordinary page editing.

Backups must include the new authoritative records along with the existing
database and full content repository. Derived views may be rebuilt only if all
inputs needed for that rebuild are retained. Restore testing must verify open
findings and completed reviews, not merely page bodies.

## 10. Experiment design

### Competing explanations

The hypothesis is supported only if Wikikiki reduces consequential mistakes or
coordination effort at comparable task quality and operating cost.

Alternative explanations to test include:

- Agents merely needed a notification and another read of the source.
- A better checkpoint template solves most of the problem.
- Explicitly recording dependencies creates more work than it saves.
- A simple adapter for an existing tool produces the same outcome.
- The apparent improvement comes from more model calls or human hints.

### Baselines

| Condition | Required behavior | Why it matters |
| --- | --- | --- |
| A. Shared Markdown | Same task template, evidence and human change notifications, with ordinary file tools | Tests whether structured records alone explain the benefit |
| B. Wiki with a small adapter | Conditional writes, source refresh before dependent steps, recorded source versions, a change diff and a simple model prompt identifying affected work | Tests against the strongest practical inexpensive alternative |
| C. Wikikiki experiment | Impact findings, review outcomes and handoff, with explicit and assisted capture measured separately | Tests the additional workflow and capture burden |

Give condition B reasonable tuning and access to relevant recorded sources.
Do not deliberately withhold a useful adapter merely to make C look better.
Before a larger product commitment, reproduce the strongest relevant workflow
in an existing memory product or research artifact where practical. Record any
installation failure or capability gap rather than scoring an untested product
as inferior.

All conditions receive the same available evidence and equivalent notice that a
source changed. They do not receive the evaluator's answer key. Keep model and
tool access, task budget and intervention rules comparable. Record any extra
inference cost used to produce impact suggestions.

Label genuinely ambiguous cases separately and preserve reviewer disagreements.
Do not force a disputed consequence into a binary answer key. Report the effect
of these cases on the result rather than quietly excluding difficult examples.

### Case families

| Family | Situation | Required observation |
| --- | --- | --- |
| Changed requirement | A premise changes while dependent work is active | Relevant work is reviewed and the final artifact satisfies the new requirement |
| Harmless edit | Wording or metadata changes without changing the requirement | No incorrect repair is introduced and review burden stays low |
| Indirect dependency | A source supports a plan that supports another artifact | The indirect relationship is found or the coverage limitation is exposed |
| Incomplete evidence | A dependency is omitted or its source becomes unavailable | Misses are scored and uncertainty is visible rather than replaced with a completeness claim |
| Disagreement | Two sources or participants conflict | Attribution and uncertainty survive the handoff until evidence resolves the conflict |
| Concurrency and restart | A source changes during review, or processing is interrupted | Stale review results are rejected and open work survives recovery |

Every family should include unaffected work to measure unnecessary interventions.
Use a simulated action for execution-race cases. Record that simulation as a
limitation rather than treating it as proof about external systems.

Include an offline client, a missed notification, a narrower exception, a change
followed by reversion and a partially repaired task among the variants. Include
an untrusted source that attempts to direct the agent's behavior, and verify that
the content gains no execution authority through an impact finding.

### Sampling and execution

1. Collect concrete correction incidents from project work. Remove secrets and
   obtain permission before using another person's material.
2. Use six to eight development cases to debug the workflow and measurement.
   Their outcomes are diagnostic and must not enter the final comparison.
3. Freeze the protocol, budget and decision thresholds before evaluation.
4. Prepare at least 24 held-out case variants across the six families. Keep
   all variants derived from the same underlying task history in one split.
5. Run each condition in fresh sessions. Counterbalance order and use matched
   variants so participants do not carry the earlier solution into a later run.
6. Record the initial work and the exact moment of the injected change. Follow
   the case through final output and a fresh-session handoff.
7. Score artifacts against a predefined rubric. Hide the condition from the
   outcome reviewer when practical and retain disputed judgments.
8. Repeat a subset with fresh sessions to assess variability. Freeze the subset
   selection before seeing which condition wins.

Repeated runs remain observations within the same workflow. Do not count them as
independent users or projects when presenting confidence in the result.

Twenty-four cases per condition can expose failures and guide investment. They
do not establish universal reliability or a market-wide performance ranking.
Report counts, variation, exclusions and failures alongside summary percentages.

### Measurements

| Measure | Definition |
| --- | --- |
| Final task success | Cases whose final artifact meets the updated requirement and preserves required unaffected behavior, divided by all attempted cases |
| Consequential stale result | A final output or simulated action materially relying on the superseded premise |
| Impact recall | Correctly identified affected work items divided by all affected items in the answer key |
| Impact precision | Correctly identified affected items divided by all items flagged as affected |
| Unnecessary intervention | Unaffected work paused, changed or sent for avoidable review because of the system's finding |
| Human coordination effort | Time spent entering relationships, correcting suggestions, explaining changes, reviewing findings and resolving follow-up |
| Repeated explanations | Unplanned human restatements of a correction after its initial entry |
| Agent and service cost | Model calls, tokens, runtime, latency and monetary cost when available |
| Recovery correctness | Open findings and review evidence retained without stale closure or duplicate effects after restart |
| Repeat use | An external participant voluntarily uses the workflow for another suitable task |

Use `unknown` for unavailable measurements. A zero denominator is `not
applicable`, not a perfect score. Timeouts, model failures and abstentions remain
in attempted-case totals. Report useful uncertainty separately from successful
repair. Setup effort and ongoing task effort should be reported separately and
also combined for the observed pilot period.

Define work-item granularity before evaluation and keep it fixed across
conditions. Splitting or merging items after seeing findings must not change the
denominators. Report per-case outcomes as well as pooled item metrics.

### Provisional decision thresholds

These are proposed investment thresholds, not measured performance or scientific
guarantees. Calibrate them on development cases, then freeze them before the
held-out evaluation. Report changes rather than moving the thresholds after
seeing results.

Continue to an external product pilot only when all of the following hold:

- Registered revision checks, stale-review rejection and restart recovery pass
  every deterministic case in their declared test scope.
- Assisted impact detection reaches at least 90 percent recall and 80 percent
  precision on the scored work items, with raw counts reported by case family.
- Final task success is at least as high as the strongest simple baseline, and
  consequential stale results are no more frequent.
- Median total human coordination time falls by at least 30 percent against the
  strongest simple baseline. Check the distribution and paired cases so that
  easy tasks do not hide costly failures.
- No more than 10 percent of unaffected work items receive unnecessary
  interventions. Review all such cases, even when the threshold passes.
- The assisted-capture condition meets the workflow criteria without a person
  writing bespoke executable premises for each task.
- Model spend and completion latency remain within ceilings chosen from the
  development cases and frozen before evaluation. Missing ceilings prevent a
  pass. Higher budgets require an explicit investment decision and a new frozen
  comparison, with the extra cost weighed against measured human-time savings.

Passing these gates supports another experiment. The sample size still limits
the strength of any conclusion, especially close differences in task success.

## 11. Continue, narrow or stop

Continue when the workflow reduces repeated coordination in held-out cases,
retains task quality and is usable by someone who did not build it.

Narrow the product when a specific task family benefits but others produce
noise. A focused tool for changing software requirements may be more useful
than a general promise about collective memory.

Stop treating this direction as the project's 0 to 1 opportunity when:

- A normal wiki with a small adapter achieves comparable outcomes and effort.
- The benefit disappears once baseline agents refresh sources properly.
- Users have to maintain most important relationships manually.
- The system creates enough irrelevant findings that people ignore them.
- Semantic inference repeatedly misses the consequences users care about.
- Maintaining enforcement requires rebuilding every agent's execution runtime.
- External users complete the demonstration but do not choose to use it again.

A failed hypothesis does not erase the value of the current application.
It changes the investment decision. Preserve useful generic improvements and
remove experimental complexity that has no demonstrated use.

## 12. Delivery sequence and review gates

The effort ranges below are planning estimates. Scope them again after the first
cases are selected. They are not delivery commitments.

| Phase | Work | Exit evidence |
| --- | --- | --- |
| 0. Frame the problem | Gather recent incidents, select case families and prepare strong baselines | Written cases with expected affected and unaffected work |
| 1. Mechanical slice | Timebox roughly three to five focused development days for explicit dependencies, impact records and one local scenario | A correction survives restart and a fresh-session handoff with inspectable attribution |
| 2. Capture experiment | Add assisted relationship suggestions and count all correction effort | Measured capture accuracy and manual burden on development cases |
| 3. Held-out comparison | Freeze the protocol and run matched trials | Complete results including failures and a continue, narrow or stop decision |
| 4. External pilot | Recruit at least three independent self-hosting users or teams and observe repeated suitable tasks | Installation and restore evidence, measured task outcomes and voluntary repeat use |
| 5. Productize | Harden only the behavior supported by evidence, document integration contracts and package a release | Reproducible setup, supported clients, recovery tests and an honest demonstration |

The existing [handoff pilot](HANDOFF_PILOT.md) provides a starting method for
fresh-session continuation. This experiment adds correction impact and stronger
baselines. The [roadmap](../ROADMAP.md) remains the broader delivery plan.

Record responsibilities before a run: the product owner chooses the investment
question, an implementation owner maintains the slice, an evaluator scores
outcomes and an independent operator tests deployment. One person may fill more
than one role, but disclose that limitation.

### First implementation task after experiment design

Create one revision-bound work record linked to one project premise. Change the
premise and show the affected work with old and new evidence. Record either a
repair or a supported finding that the work remains valid. Resume in a fresh
client and after a server restart.

Acceptance requires attribution, preservation of prior evidence, rejection of
stale review results and continued visibility of unknown or unresolved work.
No new model provider, background agent or external action is required for this
first slice. This document does not report that slice as completed.

## 13. Product position if the evidence supports it

The current honest description remains:

> A self-hosted workspace where people and agents maintain shared knowledge.

The description to earn through the experiment is:

> A shared workspace that shows how changing knowledge affects ongoing work
> and helps people and agents repair it together.

Specific evidence can eventually support narrower claims such as fewer repeated
explanations during a defined handoff workflow. Avoid claims that Wikikiki
understands all dependencies, guarantees agent behavior or offers a capability
nobody else has.

Potential lasting advantages include a low-effort workflow, dependable client
integrations, inspectable repair history and easy operation. These require
repeated use and maintenance. They are not established by a data model, the Rust
implementation, ownership of a domain or a polished demonstration.

An open, reproducible evaluation set can build trust and expose regressions.
Use synthetic cases or material whose owners have approved publication. Do not
make a product advantage depend on collecting private customer histories.

Pricing and support models remain open. Evaluate demand after users demonstrate
repeat use. The self-hosting commitment should remain clear in documentation and
in any future commercial offer.

## 14. Evidence and decision record

No trial results are recorded yet. After each evaluation, retain a concise
decision record in normal project documentation with these fields:

| Field | Required entry |
| --- | --- |
| Question | The user capability or implementation assumption being tested |
| Version | Source revision, relevant local changes, client versions and model configuration |
| Protocol | Frozen case set, conditions, budgets and scoring rubric |
| Results | Raw counts, coordination time, costs, unknowns and failed runs |
| Limits | Missing integrations, simulated effects, reviewer conflicts and coverage gaps |
| Decision | Continue, narrow or stop, with the evidence that caused the decision |
| Next test | The smallest remaining uncertainty worth resolving |

Keep sensitive raw traces outside Git. Publish only reviewed, sanitized evidence.
Record product knowledge in ordinary project documentation so that it remains
usable without access to any particular agent's session history.
