# Code standards

These rules come from what reviews keep asking for. A reviewer checks each
one; a PR that misses one gets a comment saying which.

## Tests

| Rule | What a reviewer looks at |
| --- | --- |
| A fix comes with a test that fails without it. | Revert the fix by hand, run the test, watch it fail with a real `FAIL` on the target behavior. A compile error or a skipped test doesn't count. The PR names the test and what it printed. |
| Run the full suite, not the packages you think you touched. | `cargo nextest run --workspace`, clippy with `-D warnings`, and `npm run check && npm run build` for web UI changes. |
| A test never reaches the network or the real `data/` directory. | Sources and HTTP are served from local fixtures; the database is a temp file. |
| A test that fails on `main` is a bug to fix now. | "Fails the same on main" is not a baseline. Fix it at its cause in a commit of its own, or file an issue, before shipping on top of it. |

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
- A squash merge's message is written, not GitHub's default:
  `gh pr merge --squash --subject --body-file`.
- Merge with `--match-head-commit <reviewed sha>`: a push during review
  stops the merge instead of shipping code nobody ran.
- The maintainer's standing authorization: an agent merges work it has
  verified against these criteria with CI green on the reviewed head. A
  change to a deliberate product decision, or a PR stopped by the
  harm-first check, still waits for the maintainer's explicit word.

## Releases

- The workspace `Cargo.toml` version is the trigger: a push to `main` that
  changes it runs the release pipeline. Don't push tags or create releases
  by hand.
- A release counts only when the run is green, `gh release view` shows all
  4 platform archives, and Docker Hub's and ghcr.io's `latest`, `X.Y.Z` and
  `X.Y` tags point at the new multi-arch manifest. Verify before naming the
  version to anyone.
- A cancelled or failed release run is not a released version. Check
  `git ls-remote --tags` and `gh release list`, then let the next green run
  carry it; the version job skips only when the release exists.

## Reviewing a PR

- Run it. Reading the diff alone isn't a review.
- Look for harm first: a PR that touches `.github/`, the Dockerfile,
  dependencies, or anything that sends data somewhere new stops the review
  until the maintainer has looked.
- Break the fix by hand and watch the new test go red.
- Check the diff against the PR's stated goal; anything outside it goes in
  a PR of its own.
- Docs match the source: a reference that no longer resolves is a bug in
  the change that broke it.
