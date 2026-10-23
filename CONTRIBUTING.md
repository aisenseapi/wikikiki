# Contributing

Wikikiki is an early self-hosted workspace for people and agents. Read
[README.md](README.md) for current behavior, [ROADMAP.md](ROADMAP.md) for
priorities and [AGENTS.md](AGENTS.md) for repository rules.

## Useful contributions

Run the [external handoff pilot](docs/HANDOFF_PILOT.md) or follow
[the installation and restore guide](docs/SELF_HOSTING.md) on a fresh machine.
Report what failed, where help was needed and the exact source revision.
Negative results are useful evidence.

Code contributions should solve an observed problem in installation, reliable
collaboration or operations. New retrieval and memory features need a comparison
against current behavior and an existing alternative. A proposed feature is not
novel merely because Wikikiki has not implemented it.

## Checks

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

Run the sync-client tests documented in scripts/tests when changing that client.
CI checks formatting, lint and tests. Read tests/page_consistency.rs before
changing persistence. Preserve correct actor/content attribution, conditional
updates, cancellation handling and replay of interrupted operations.

Add a regression test for a reproduced bug or a new behavioral guarantee.
Avoid tests that only restate implementation details. Use isolated temporary
databases and repositories, never an operator's running instance.

## Documentation

The README and current implementation section of DESIGN.md describe the product
contract. Update them with behavior changes. Keep future proposals explicitly
marked and record the practical limits of a guarantee. Do not claim automatic
recovery, access control or integration behavior that has not been implemented
and tested.

## Git and licensing

Preserve unrelated local work. Review explicit files before staging. Do not add
credentials, local state or tool attribution to Git. Do not create commits or
pushes unless requested. Follow the existing valid project identity without
inventing or changing an identity to make a commit succeed.

Contributions are licensed under MIT or Apache-2.0, at the user's option.
See [LICENSE-MIT](LICENSE-MIT) and [LICENSE-APACHE](LICENSE-APACHE).
