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

## Public writing

Docs, lessons, and issue/PR text record mechanisms and numbers, never
local machine details: no hostnames, absolute paths, internal domains
or IPs, and no how-or-where a tool was installed on a private machine.
Do not name or quote the maintainer or relay their private messages in
public writing. Describe technical decisions directly, without personal
attribution.

## Commands

- Server: `cargo nextest run --workspace` and
  `cargo clippy --workspace --all-targets -- -D warnings`
- Web UI: `cd webui && npm ci && npm run check && npm run build`
- When `webui/` changed, also `cargo build --release -p sumidoku-server`:
  rust-embed only bundles `webui/build` into a release binary.

## Layout

- `crates/server` — axum HTTP server: REST `/api/v1` and embedded web UI
- `crates/wasm` — Aidoku WASM source runtime (wasmi)
- `vendor/aidoku` — vendored upstream crate; it tracks upstream, do not
  restyle or "fix" it locally
- `webui` — SvelteKit (Svelte 5) SPA with shadcn-svelte + svelte-i18n
  (en/zh/fr/it), built to `webui/build`; `npm run check` is `svelte-check`
- `data/` — local dev database, cover cache and sources; untracked, never commit it

## Release

- The version lives in the workspace `Cargo.toml`. A push to `main` that
  touches it runs the release pipeline: four platform archives, Docker Hub
  and ghcr.io tags `latest`/`X.Y.Z`/`<major>.x` (e.g. `0.x`), and the
  GitHub release.
- Never push a tag or create a release by hand; CI does both.
- A release counts only when the run is green, the release shows its 4
  assets, and the 3 tags moved on both registries. Verify before
  announcing; the checklist is in
  [docs/code-standards.md](docs/code-standards.md#releases).

## Session ritual

Record a lesson the moment work teaches one — don't save it for later.
Each entry in [LESSONS.md](LESSONS.md) goes in as its own commit: the
rule, why, and the evidence. A scheduled daily review is the backstop: it
catches anything the day should have recorded but didn't, counts
recurrences, and moves a lesson seen three times into
[docs/code-standards.md](docs/code-standards.md).
