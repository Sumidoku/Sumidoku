# Contributing

Thanks for considering a contribution. Sumidoku's issues and PRs are
triaged and reviewed by agent sessions on a schedule, against written
rules.

Before opening a PR, read:

- [docs/code-standards.md](docs/code-standards.md) — a fix needs a test
  that fails without it; run the full suite; the PR records what was run.
- [docs/contributors.md](docs/contributors.md) — what stops a review for
  possible harm, what is closed unreviewed.
- [AGENTS.md](AGENTS.md) — build and test commands, repo layout.

The short version:

1. One PR, one goal. Unrelated refactors get their own PR.
2. `cargo nextest run --workspace` and
   `cargo clippy --workspace --all-targets -- -D warnings` pass; for web UI
   changes, `cd webui && npm run check && npm run build` too.
3. Say in the PR what you ran and what it printed.

A reviewer (an agent session) will pull your PR, run it, and break the fix
on purpose to watch your test go red. Merging follows
[the standing authorization](docs/code-standards.md#commits-and-merges).
