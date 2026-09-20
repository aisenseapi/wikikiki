# Contributing

Wikikiki is early. The foundation is deliberate and fairly well tested; most of
the product described in [DESIGN.md](DESIGN.md) is not built yet. That makes it
a good project to join and an easy one to misread, so this file says where the
edges are.

## Read these first, in this order

1. **[README.md](README.md)** — what runs today, and what it explicitly does
   not guarantee. The limitations there are load-bearing, not modesty.
2. **[ROADMAP.md](ROADMAP.md)** — what is unbuilt, in what order, with
   completion criteria. This is the closest thing to a list of open work.
3. **[AGENTS.md](AGENTS.md)** — the repository's working rules. They apply to
   people and to agents; most of them are about not destroying existing work.
4. **[DESIGN.md](DESIGN.md)** — the full architecture, including the reasoning
   behind decisions and the questions still open. Long, but it answers "why is
   it like this" for almost everything.

## What would actually help

The useful contributions right now are not features. In rough order:

- **Running the [handoff pilot](docs/HANDOFF_PILOT.md)** and reporting what
  happened. The pilot asks whether shared memory measurably reduces the work
  of keeping several agents and a human aligned. Nobody has run it. Results —
  including negative ones — are worth more than code at this stage.
- **Recovery after a crash between the Git commit and the SQLite transaction.**
  These are two durability boundaries and no lock spans them. It needs an
  operation journal and a reconciliation pass at startup. This is the last
  structural gap in write integrity.
- **Correction propagation** (ROADMAP M4): recording which conclusions used
  which sources, so a correction can mark what it invalidates. This is the
  part of the design that is not already available elsewhere.
- **Clearing the lint backlog.** `cargo fmt` and `cargo clippy` do not pass.
  CI reports them without blocking. Fixing them is welcome; reformatting files
  you were not otherwise touching is not, because it buries the real change.

## Before you open a pull request

```sh
cargo test --locked
```

That is the whole suite, including the concurrency, cancellation and
conditional-write regressions. CI runs it on every pull request.

If you are changing the write path, read `tests/page_consistency.rs` first. It
asserts that concurrent writers cannot cross attribution, that an accepted
write survives its caller disconnecting, and that invalid input changes
nothing. Those properties were each broken at some point and each cost real
debugging. If a change makes one of those tests awkward, the change is
probably wrong.

New behaviour should come with a test that fails without it. A test that
passes both before and after proves nothing.

## Documentation is part of the change

README.md and DESIGN.md make specific claims about what the code does and does
not do. If your change makes one obsolete, update it in the same commit. If
your change cannot deliver what a document promises, say so there rather than
leaving the promise standing.

## Attribution

Commits in this repository are authored as `wikikiki <wikikiki@aisense.no>`,
and no tool or agent identifies itself in Git metadata or commit messages. See
[AGENTS.md](AGENTS.md#git). This is about keeping the history a record of what
changed rather than of who or what typed it; it is not a claim on your work.

## Licence

By contributing you agree that your work is licensed under the same terms as
the project: MIT or Apache-2.0, at the user's option. See
[LICENSE-MIT](LICENSE-MIT) and [LICENSE-APACHE](LICENSE-APACHE).
