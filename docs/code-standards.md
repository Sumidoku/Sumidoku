# Code standards

These rules come from what reviews keep asking for. A reviewer checks each
one; a PR that misses one gets a comment saying which.

## Tests

| Rule | What a reviewer looks at |
| --- | --- |
| A fix comes with a test that fails without it. | Revert the fix by hand, run the test, watch it fail with a real `FAIL` on the target behavior. A compile error or a skipped test doesn't count. The PR names the test and what it printed. |
| The break-and-restore starts from a committed green state. | The green state is committed before the fix is broken; the restore is `git restore -- <file>` or an equivalent one-file revert — never a bare `git stash` among uncommitted work, and `git stash drop` is not a restore. |
| Run the full suite, not the packages you think you touched. | `cargo nextest run --workspace`, clippy with `-D warnings`, and `npm run check && npm run build` for web UI changes. |
| Search transitions are separated by data source and gesture. | Only local data searches live; remote source calls and search-view navigation belong to explicit submission. Restored searches wait for required source configuration, and clearing input explicitly clears submitted results. Keep drafts, submitted queries and view state separate; filters are available before the first submission, and the same source-specific filter-drop rule applies to fresh and restored requests. |
| Event-driven state has one owner for each transition. | A request's success continuation must not reapply optimistic state that an event may already have superseded. Keep it only when the same ordering channel guarantees it is still current, and exercise the fast-completion path in a real browser. |
| A test never reaches the network or the real `data/` directory. | Sources and HTTP are served from local fixtures; the database is a temp file. |
| A test that fails on `main` is a bug to fix now. | "Fails the same on main" is not a baseline. Fix it at its cause in a commit of its own, or file an issue, before shipping on top of it. |

## Database migrations

| Rule | What a reviewer looks at |
| --- | --- |
| Schema changes are new numbered files only; do not edit an applied migration. | sqlx checksums the whole file against `_sqlx_migrations` at startup. `db.rs:shipped_migrations_are_immutable` pins the canonical checksums. The explicitly registered historical Chinese/English LF/CRLF variants are a closed compatibility set; accepting any other old-file edit or deriving a replacement checksum from migration text is a stop. |
| No SQL triggers or views without a special case. | What a write must also do belongs in the code that makes the write, and sync payloads belong in `sync/journal.rs`, where changing one needs no migration. `db.rs:migrated_schema_has_no_triggers_or_views` fails on either; an exception states in its PR why code cannot do it and names the object in that test. |
| Migration comments are English and files use LF. | Keep comments ASCII in `crates/server/migrations/*.sql`; `db.rs:shipped_migrations_use_ascii_comments_and_lf` rejects other encodings and CR/CRLF bytes. `.gitattributes` forces LF checkouts. |

## Acceptance criteria

A change is merged only when it shows each of these. A check that wasn't
run is listed as not run, never left out.

1. The test above, red without the change and green with it.
2. The full suite locally: nextest and clippy, plus the web UI checks when
   `webui/` changed.
3. CI green on the reviewed head.

## Commits and merges

- Commit messages are English and conventional (`feat:`, `fix:`, `chore:`),
  naming the area: `server: …`, `webui: …`, `ci: …`.
- The body records what was run. A large change with no body can't be
  reviewed afterwards.
- One commit, one goal. Unrelated refactors go in a PR of their own.
- A squash merge's message is written, not GitHub's default, and carries
  no `Co-Authored-By` trailers: `gh pr merge --squash --subject --body-file`.
- Merge with `--match-head-commit <reviewed sha>`: a push during review
  stops the merge instead of shipping code nobody ran.
- Take a fresh look right before merging — as its own step, never
  chained with the merge (`check && merge` makes the check a log line):
  `gh pr view --comments` for comments and commits that arrived after
  acceptance. Anything new means reviewing that delta first, not merging
  on a stale read.
- The maintainer's standing authorization: an agent merges work it has
  verified against these criteria with CI green on the reviewed head. A
  change to a deliberate product decision, or a PR stopped by the
  harm-first check, still waits for the maintainer's explicit word.

## Releases

- The workspace `Cargo.toml` version is the trigger: a push to `main` that
  changes it runs the release pipeline. Don't push tags or create releases
  by hand.
- The agent decides when a release is due (maintainer's authorization):
  weigh what has landed on `main` since the last tag. Cut it as its own PR
  that only bumps `workspace.package.version` — breaking → major,
  features → minor, fixes → patch — and let CI publish on merge.
- A release counts only when the run is green, `gh release view` shows all
  5 platform archives, and Docker Hub's and ghcr.io's `latest`, `X.Y.Z` and
  `<major>.x` (e.g. `0.x`) tags point at the new multi-arch manifest.
  Verify before naming the version to anyone.
- A cancelled or failed release run is not a released version. Check
  `git ls-remote --tags` and `gh release list`, then let the next green run
  carry it; the version job skips only when the release exists.

## Working an issue

- Read the whole thread before deciding anything; deciding from the body
  alone has caused reopens.
- Re-read the thread right before claiming the work: a late comment can
  change the scope or answer the question.
- A comment on a closed issue or a merged PR still deserves an answer.
  The updated-since scan with `--state all` surfaces them.

## Reviewing a PR

- Run it. Reading the diff alone isn't a review.
- Web UI style is reviewed manually because no formatter runs. Compare
  changed blocks with sibling indentation and watch for glued imports;
  TypeScript does not catch structure drift.
- Look for harm first: a PR that touches `.github/`, the Dockerfile,
  dependencies, or anything that sends data somewhere new stops the review
  until the maintainer has looked.
- Break the fix by hand and watch the new test go red.
- Check the diff against the PR's stated goal; anything outside it goes in
  a PR of its own.
- Docs match the source: a reference that no longer resolves is a bug in
  the change that broke it.
