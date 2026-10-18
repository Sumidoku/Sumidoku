<!-- reviewed-through: 7264734 (2026-10-18) -->
# Lessons from merged work

Sumidoku's code is written, reviewed and released by agent sessions. What
went wrong is kept here as a rule, then why, then the evidence. Read this
before changing code. A lesson seen again on a later day moves into
[docs/code-standards.md](docs/code-standards.md).

**A CDP connection is bound to the runtime that first polled it; drive the
connect on the host runtime.** tokio I/O handles register with the reactor
active at creation/poll time, so a `Browser::connect` driven inside a
client-owned `current_thread` runtime cannot be driven from anywhere
else — when the handler and screencast pump were moved to the server
runtime (needed because a login session is idle between calls, and a
current_thread runtime is only polled during `block_on`), frames arrived
exactly once inside a `block_on` window and every later command hung with
no error. When a connection must outlive `block_on` windows, spawn the
connect itself on the host runtime. Two neighbours of the same bug: a
client that owns a `Runtime` panics in tokio's shutdown assertion if
dropped in async context, so every exit path must release it inside
`spawn_blocking`; and headless Chromium only emits screencast frames on
damage, so starting a screencast does not guarantee an initial frame —
nudge one screenshot.
- Evidence: 2026-10-19, issue #115's real-browser smoke: the pump logged
  one frame (produced by a command inside a `block_on` window), then
  `screencastFrameAck` and every later command hung; moving the connect
  onto the app runtime delivered frames end-to-end. The runtime-drop panic
  surfaced in the same probe (tokio shutdown.rs:51).

**`tokio::time::pause` breaks sqlx pool acquisition — age test data
directly instead of freezing the clock.** With `start_paused`,
`SqlitePool::connect` fails with `Db(PoolTimedOut)` before the first
query, so a paused-clock test cannot even build its database. When a
test needs an aged in-memory timestamp, set it through the struct
(same-module tests can reach private fields) and keep loop-level tests
on a short real tick.
- Evidence: 2026-10-19, PR #128's idle-evict tests: all five
  `start_paused` state tests died at `db::create_pool` with
  `Db(PoolTimedOut)`; rewinding `SourceEntry.last_used` via a
  `make_idle` helper made them deterministic with no clock control.

**Expected values come from running the reference engine, not from
reasoning about it.** When the contract is "match engine X" (for
html_dom: jsoup/SwiftSoup), an expected value produced from docs or
memory can be wrong in either direction — here an issue's carefully
researched expectation still didn't survive contact with the real
engines. Before asserting against the Aidoku contract, run the actual
jsoup/SwiftSoup on the fragment and pin what they print; when the two
engines disagree, Aidoku's engine (SwiftSoup) wins and the divergence
gets a pin test naming the choice.
- Evidence: 2026-10-18, issue #83's B2 quoted `"xy"` for
  `x<script>…</script>y`; jsoup 1.15.3 and SwiftSoup 2.13.7 both produce
  `"x y"` (the script element is a block separator; its content is what
  is dropped), and `"xy"` is that fragment's `ownText()`. PR #95 asserts
  the measured value.

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

**Write every GitHub body to a file; never inline it.**
Backticks inside a double-quoted `--body`/`--comment` string are command
substitution to the shell: the text between them ran as a command and the
posted comment had holes where `` `IN (...)` `` used to be. Same class as
the long-standing body-file rule for merge requests, and it bites on plain
issue comments too.
- Evidence: 2026-10-18, the claim comment on issue #33 went out mangled;
  fixed via comment PATCH with `--body-file`.

**A 200 on a config write proves nothing; read the parsed value back.**
A settings PUT stored the whole `{"values": …}` envelope as the values
map, so the consumer's top-level key never existed and the knob silently
stayed at its default — every "progressive" screenshot then showed the
final state. The read-back (`values.values.partial_delay`) was the only
place the bug was visible. Write, then parse what the consumer will parse.
- Evidence: 2026-10-18, the #50 stream measured 0ms until the flat re-PUT;
  after it the same request took the expected 16s.

**Write contract docs from the pipeline file, not from memory of the last
run.** *(recurrence of the standing "copy facts from the authoritative
source" rule — noted here so the daily review can count it.)*
The release docs said the floating image tag was `X.Y` because that is how
the previous verification was summarized; the workflow has always
published `<major>.x`. Three documents and the patrol cron's check
inherited the wrong tag, and it survived a whole release cycle.
- Evidence: PR #51 (2026-10-18) corrected all three after the v0.2.0
  manifest query showed `0.x`, not `0.2`.

**A smoke server started from the wrong directory serves stale assets.**
`webui_dir` resolves `./webui/build` against the process's cwd; starting a
PR worktree's binary from the main checkout's directory serves the main
checkout's old UI, and the page looks plausibly "done". Before trusting a
smoke page, curl the served asset hash and compare it with the build you
mean to test.
- Evidence: 2026-10-18, the #38 smoke served `index-BWzzabJ3.js` while the
  worktree build was `index-BnNwpN81.js`; restarting with the worktree cwd
  served the right one.

**Count sqlx statements from the event's `summary` field, and filter
PRAGMA.** sqlx emits one `sqlx::query` event per statement with the
summary in the `summary` field (not `message`), and connection setup emits
PRAGMA events that fall into the measurement window on slow machines. A
counter reading `message` and keeping PRAGMA was green locally and flaky
red in CI.
- Evidence: PR #39's first CI run red while local was green; the fixed
  counter ran 60/60 locally and green in CI (2026-10-18).

**The build matrix is CI's job; never rehearse it locally.**
A dispatch prompt asked the subagent to prove the four release targets
locally, so it burned ~50 minutes on a `cross` + docker compile that the
CI legs already cover — on a machine with a history of memory-pressure
OOM kills. Before writing a verification requirement into a prompt, ask
whose job it is: anything the build/docker/release legs already answer
belongs to CI; local work is tests plus the host build. And when a
dependency choice carries build risk, prefer the zero-risk option (pure
Rust) so cross uncertainty never materializes at all.
- Evidence: 2026-10-18, the #21 dispatch's local cross build was killed
  on the maintainer's ask; the instruction was corrected to CI-owned
  verification and pure-Rust-first selection.

**Decide an issue only after reading the whole thread, not just the
body.** `gh issue view --json body` returns no comments, and a decision
made from the body alone can contradict what the maintainer already said
in the thread. Here a references comment (two headless-browser options)
sat unread for five hours while the issue was decided and merged the
other way; a collaborator had to reopen the issue to get it re-examined.
Read `gh issue view --comments` before deciding or closing anything, and
treat a reopen as new input, not noise.
- Evidence: 2026-10-18, issue #26 was decided "sandbox + explicitly
  unsupported" and merged while everpcpc's chromiumoxide/moli references
  sat unread in the thread; dyphire reopened it the same day.

**A rewritten branch gets its test count re-checked, not just its CI.**
A collaborator consolidated their branch into one commit before merge;
the rewrite kept the code fixes but silently dropped two tests added
during review. CI went green because the suite got smaller, and
`--match-head-commit` matched the new head while the content had
changed. After any force-push that rewrites history, diff the test list
(`cargo nextest list` or the suite count) against the reviewed state
before merging — the commit-sha pin proves where the head points, never
what it contains.
- Evidence: 2026-10-18, PR #61's rewrite into `24971ab5` dropped the two
  jar sink tests from the review fix; re-added in PR #64.

**A probe on the app's own port answers the dev server, not your
container.** The main checkout's dev server listens on :3000, so a test
sidecar published with `docker run -p 127.0.0.1:3000:3000` looks healthy
(docker prints a container id) while every probe to 127.0.0.1:3000 lands
on the dev server. Verify a probe against what the sidecar should
actually say — a CDP websocket upgrade answers 101, not the webui's
HTML — and pick a host port that is really free before blaming the
client library.
- Evidence: 2026-10-18, PR #62's e2e setup: browserless on :3000
  answered every handshake with the Sumidoku webui ("HTTP error: 200
  OK" from tungstenite); republishing on 127.0.0.1:3300 made the CDP
  handshake succeed on the first try.

**`&Request` held across `.await` fails the `FromFn: Service` bound without
naming `Send`.** axum's `Body` wraps `UnsyncBoxBody` — it is `Send` but
not `Sync` — so a `&Request` captured by an `async` middleware's future
makes that future non-`Send`, and rustc reports the whole
`FromFn<…>: Service<_>` bound as unsatisfied with no pointer to the real
cause. Extract owned data from the request (headers, tokens, query)
*before* the first `.await`; borrows of `HeaderMap` are fine because it is
`Sync`. When a middleware that "looks just like the working one" fails
this bound, strip the body to `next.run(req).await` and re-add pieces —
that bisect lands on the non-`Send` capture in minutes.
- Evidence: 2026-10-18, the auth middleware kept `&req` alive across
  `bearer_granted(...).await`; two identical-looking signatures (working
  `basic_auth` vs failing `require_auth`) differed only in that borrow,
  and the fix was passing an owned `Option<String>` token instead.

**A new field on a public struct breaks every out-of-crate struct literal;
list them before pushing.**
`SourceConfig.accept_language` compiled inside the crate and broke all
seven `crates/wasm/tests/*.rs` integration tests that build
`SourceConfig { … }` by hand. A parse-only check scoped to the files you
edited cannot see that, and no local compile was possible
(wasmer-compiler-cranelift is a `compile_error!` on Windows), so the red
landed in CI instead of the session. Before pushing a struct change, grep
the whole workspace — `crates/*/tests/` included — for `Name {` literals
and update every one.
- Evidence: 2026-10-18, branch `net`, the "send the source language's
  Accept-Language" commit: CI's `cargo test --workspace` reported `E0063:
  missing field accept_language` in rekey_host, js_host, webview_host,
  canvas_host, html_mutation, partial_result and smoke; the follow-up
  commit adds the field to each.

**Never run `cargo fmt` in this repo; the tree is hand-formatted with tabs.**
There is no `rustfmt.toml`, so default rustfmt expands every tab to spaces
and rewrites the whole workspace — 108 files, `vendor/aidoku` included,
which `AGENTS.md` says must not be restyled. A single `cargo fmt --all`
produced a 31k-insertion/28k-deletion diff over tracked files; only
`git reset --hard` undid it. Match the surrounding indentation by hand;
CI checks clippy and tests, not formatting.
- Evidence: 2026-10-18 session, `cargo fmt --all` reformatted 108 files
  (including `vendor/aidoku`) before the fix was re-applied on the reset
  tree.

**The pre-merge re-check is its own step; the merge runs only after the
result is read.** A fresh-look that shares a shell command with the merge
(`check && merge`) is a log line, not a gate — the merge fired seven
minutes after a collaborator's explicit hold, un-read. Run the check,
look at the output, then decide; never chain the two.
- Evidence: 2026-10-18, PR #103 (release 0.5.0) merged at 15:50:13 with
  dyphire's "wait until the work is finished" (15:43:15) sitting in the
  thread; the release run was cancelled 16s in — no tag, no release —
  and 0.5.0 is held for his fixes.

**A Cloudflare clearance is bound to the client that solved it; a bypass
that replays it from another client silently re-challenges.**
`cf_clearance` is tied to the solving browser's user agent (and IP), so a
solver API's response must be replayed with that same user agent — the
relay and Suwayomi both do. Sumidoku routed a challenged request through
trawl once and then sent the cookie back over its own reqwest client with a
different (Chrome) user agent against trawl's Camoufox Firefox, so every
later request re-challenged. The fix keeps a challenged host on trawl, which
holds the clearance and user agent together. Check how a solver's output is
consumed before trusting a "transparent" handoff.
- Evidence: 2026-10-19, bakamh's managed challenge worked through
  aidoku-cf-relay and Suwayomi but not Sumidoku; `routed.rs:trawl_escalation`
  now marks the host and `route_for` keeps it on trawl.

**A change event's `currentTarget` is dead by the time a deferred state
updater runs — read the value eagerly.** React nulls `currentTarget` when
the dispatch ends, so `setState((cur) => … e.currentTarget.value …)`
crashes with "Cannot read properties of null (reading 'value')" whenever
the updater is not eagerly evaluated during dispatch (batching defers
it). Grab `const v = e.currentTarget.value` first and close over `v`;
sites that survive on React's eager-state optimization are still
landmines. Type checking cannot see this — only an exercised UI catches
it, so UI changes get a real-browser smoke, not just `tsc`.
- Evidence: 2026-10-19, PR #112's localStorage input crashed the source
  settings modal on its first keystroke in real-browser smoke (fixed in
  ec3a91b); the same latent pattern in `SettingsPage.tsx` and
  `MangaDetailPage.tsx` filed as #113.

**Local UI smoke runs the dev loop, not the embedded build.** The vite
dev server proxies `/api` and `/opds` to a debug backend (README
documents it), giving hot reload and readable stacks; the embedded
production build costs a rebuild per edit, fights browser caches, and
hides stack traces behind minification — and its serving path is
already smoked by CI's docker leg on every PR. Reach for the release
binary only when the change touches the embed/serving layer itself.
- Evidence: 2026-10-19, PR #112's acceptance burned cycles on a stale
  embed (the disk branch wins over embed when `./webui/build` exists
  under the server's cwd), on cache busting, and on hand-decoding a
  minified `currentTarget` stack that dev-server source maps would have
  named directly; README.md:101 had the dev loop documented all along.

**chrono's `NaiveDateTime::parse_from_str` silently drops a parsed zone
offset — route offset-bearing formats through `DateTime`.** A format
containing `%z` parses fine into a `NaiveDateTime`; the offset is
discarded and the timestamp comes out wrong by however much the string's
zone differed from the one you attach afterwards. Any layer translating
format strings with zone tokens must branch those formats to
`DateTime::parse_from_str`. Two sibling traps from the same probe: `%f`
parses a nanosecond *count* while `%.f` parses a fraction of a second,
and only `%#z` accepts `Z`, `+09`, `+0900` and `+09:00` alike (`%z` and
`%:z` reject `Z` and the hours-only form).
- Evidence: 2026-10-19, issue #110's parse_date work; the chrono 0.4.45
  probe printed `Ok(2024-01-15T12:00:00)` for input `…+0900` under
  `%…%z` (the correct reading needs the −9h shift), and `500` under `%f`
  became 500ns rather than half a second; PR #124 routes zone-token
  formats through `DateTime::parse_from_str` and maps `S` to `%.f`.
