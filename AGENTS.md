# Notes for coding agents

Sumidoku is built and maintained by agent sessions. This file is the entry
point; the documents it links are binding.

## Before changing code

- Read [LESSONS.md](LESSONS.md): what recent work got wrong and the rule
  that would have caught it.
- Read [docs/code-standards.md](docs/code-standards.md): test, commit, merge
  and release rules.
- For a subsystem change, read the owning section of
  [docs/architecture.md](docs/architecture.md) and verify it against the
  linked source. Update that doc in the same PR when the change alters a
  contract it describes.

## Community issues and PRs

Before reviewing a community PR, read
[docs/contributors.md](docs/contributors.md): what stops a review for
possible harm, what is closed unreviewed, and how a block is proposed.
Scheduled agent sessions sweep open issues and PRs: they triage, review,
and start work on the ones that fit the project's direction, and run a
daily review of the day's merged work into
[LESSONS.md](LESSONS.md). Merging follows the standing authorization in
[docs/code-standards.md](docs/code-standards.md#commits-and-merges);
blocking stays with the maintainer.

## Commands

- Server: `cargo nextest run --workspace` and
  `cargo clippy --workspace --all-targets -- -D warnings`
- Web UI: `cd webui && npm ci && npm run check && npm run build`
- When `webui/` changed, also `cargo build --release -p sumidoku-server`:
  rust-embed only bundles `webui/build` into a release binary.

## Layout

- `crates/server` — axum HTTP server: REST `/api/v1`, OPDS `/opds/v1.2` and
  `/opds/v2`, embedded web UI
- `crates/wasm` — Aidoku WASM source runtime (wasmer)
- `vendor/aidoku` — vendored upstream crate; it tracks upstream, do not
  restyle or "fix" it locally
- `webui` — Vite + React SPA, built to `webui/build`
- `data/` — local dev database, cover cache and sources; untracked, never commit it

## Release

- The version lives in the workspace `Cargo.toml`. A push to `main` that
  touches it runs the release pipeline: four platform archives, Docker Hub
  and ghcr.io tags `latest`/`X.Y.Z`/`X.Y`, and the GitHub release.
- Never push a tag or create a release by hand; CI does both.
- A release counts only when the run is green, the release shows its 4
  assets, and the 3 tags moved on both registries. Verify before
  announcing; the checklist is in
  [docs/code-standards.md](docs/code-standards.md#releases).

## Session ritual

When a session that merged work ends, review what had to be fixed again,
what merged that shouldn't have, and what was verified too thinly. Record
what you learn in [LESSONS.md](LESSONS.md), in a commit of its own. A
scheduled session runs the same review over each day's merged work; a
lesson seen again moves into
[docs/code-standards.md](docs/code-standards.md).
