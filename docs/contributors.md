# Community PRs: triage, review, and blocks

Sumidoku is a community project whose PRs and issues are handled by agent
sessions. Two things keep that from wearing the project down: a PR that
could harm users is caught before any review of what it does, and an author
who keeps sending work that costs more to review than it gives is stopped.

## Collaborators

everpcpc (the maintainer) and dyphire are the project's main
collaborators. This only tells the reviewer who is who: their PRs get the
same run-it-and-break-it review as anyone else's, and nothing on this page
applies any less to them.

## Agent triage duty

Scheduled agent sessions sweep open issues and PRs (see
[AGENTS.md](../AGENTS.md)). For each new item the session:

- answers what it can answer, asks for what is missing, or files the fix as
  a draft PR of its own;
- reviews PRs against [code-standards.md](code-standards.md): run it, break
  the fix by hand, check the diff against the stated goal, and post the
  conclusion on the PR;
- closes what "Close without a full review" below covers, with a one-line
  reason;
- proposes blocks to the maintainer with evidence.

**An agent merges per the maintainer's standing authorization (see
[code-standards.md](code-standards.md#commits-and-merges)); a PR stopped
by "Look for harm first" always waits. Blocking an author always needs the
maintainer's explicit word.**

## Look for harm first

Before running a PR, read its diff for anything that reaches past its
stated goal. Any one of these stops the review until the maintainer has
looked:

- It changes `.github/` (workflows, actions, permissions, secrets), the
  Dockerfile, or the release flow.
- It adds a network address, an upload, telemetry, or sends a credential,
  token, cookie or file anywhere it didn't go before.
- It reads or writes a credential file or another app's config the goal
  doesn't need.
- It adds a dependency, a `replace` in `Cargo.toml`, an npm install script,
  a binary, a minified or encoded blob, or code built from a string at run
  time.
- It adds a relay, reseller, affiliate or advertisement address.

Say which line stopped it in the review. Don't run the PR's code on a
machine with real accounts until the maintainer has cleared it.

## Close without a full review

Close with a short reason, without running it, a PR that:

- changes what the maintainer has ruled on;
- is a resubmission of a PR closed for its idea, with nothing new to answer
  the reason it was closed;
- is one of several near-identical PRs opened seconds apart;
- is spam or machine-made filler.

## Blocking an author

An author is proposed for the block list when any one of these holds:

1. Three or more PRs closed as low-value (wrong, untested, unrelated,
   machine-made filler), and fewer than 30% of their PRs merged.
2. They resubmitted a PR after an explicit rejection, twice.
3. Anything deceptive: a change hidden outside the PR's stated goal, a
   faked test or verification, or anything from "Look for harm first" done
   on purpose.
4. PRs in machine bursts (several within a minute) that are low-value as
   in 1.

An agent only proposes. It writes the evidence (PR numbers, dates, what was
wrong) to the maintainer; the maintainer decides. A blocked author's new
PRs and issues are closed with: "Thanks — this account's contributions
aren't accepted in this repository." Nothing more is argued.

An author being watched (one or two low-value PRs, no pattern yet) gets a
normal review; the reviewer notes the PR here if it is low-value too.

## Lists

Blocked: none yet.

### Watched

| Author | Check | Evidence |
| --- | --- | --- |
