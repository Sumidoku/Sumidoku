<!-- reviewed-through: 351893f (2026-10-18) -->
# Lessons from merged work

Sumidoku's code is written, reviewed and released by agent sessions. What
went wrong is kept here as a rule, then why, then the evidence. Read this
before changing code. A lesson seen again on a later day moves into
[docs/code-standards.md](docs/code-standards.md).

**A workaround names its cause and gets retested when the toolchain moves.**
`strip` was disabled for every release build with no reason on record. The
real cause was rust-lang/rust#157750 (mis-aligned LINKEDIT crashing macOS
binaries, rust 1.95–1.97), fixed upstream in 1.98. The workaround outlived
the bug by two toolchain releases, and only a session asking why the
binaries were unstripped found it. A workaround without its issue number is
invisible to the person who could remove it.
- Evidence: PR #2 restored `strip = true` after the toolchain moved to
  1.99.1; the release binary runs and proc-macro dlopen works.

**A runner label is a dependency; track its retirement.**
The first v0.1.0 release run queued its Intel macOS leg on `macos-13` for
over 30 minutes until GitHub cancelled it — the image was already retired —
taking the docker and release jobs with it. The fix (cross-compile
x86_64-apple-darwin on macos-latest) landed in the same window, and the
next push's run carried the release. When GitHub announces a retirement on
actions/runner-images, grep the workflows for the label that day.
- Evidence: run 39078788016 cancelled on the queued macos-13 leg; PR #2's
  comment in release.yml now names macos-13 as deprecated.

**A rustup directory override silently pins the toolchain.**
`rustup update` reported stable current, but a directory override kept this
checkout on 1.97.1 — the release profile change was being verified against
a compiler two releases old. `rustc -vV` in the project directory is the
only reading that counts; `rustup default` says nothing when an override
exists.
- Evidence: 2026-10-18 session, `rustup show` listed the override; removing
  it moved the build to 1.99.1.

**A cancelled release run leaves no partial state — check before redoing
anything by hand.**
The version job skips only when the release already exists, so a later
green run picks the release up on its own. `git ls-remote --tags` and
`gh release list` give the real state; a cancelled run has created neither
tag nor release.
- Evidence: run 39078788016 cancelled with zero tags and zero releases
  created; run 39080251858 then ran the full pipeline for the same version.

**`gh run watch --exit-status` exits 1 on `cancelled`, not only on
`failure`.** A watcher that treats nonzero as "the build failed" misreads a
cancelled run. Read the per-job conclusion before deciding what happened.
- Evidence: the run-1 watcher exited 1 while four of five build legs were
  green; the queued macos-13 leg's cancellation was the only red.
