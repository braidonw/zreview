# Asynchronous session loading

## Goal

Open the window before any Git or GitHub work starts, keep that work off the UI
thread, and turn failures into something the reviewer can act on inside the app.

This closes the gap PLAN section 5 states plainly — no synchronous subprocess work
on the UI thread — and the error-surface requirement in section 4.

## What it replaced

Everything used to happen before `Application::run`: `gh auth`, metadata, the
namespaced fetch, one `git diff` per changed file, then the paginated comment
fetch. A large pull request meant tens of seconds with no window at all, and any
failure printed to a terminal the reviewer may not have been looking at, then
exited non-zero.

Loading `BurntSushi/ripgrep#3488` takes about four seconds. That was four seconds
of nothing.

## Shape

```text
main            parse arguments only, then open the window
  │
  ▼
SessionView     Loading → Ready | Failed          (crates/ui)
  ▲
  │ published on a timer
loading.rs      background load, foreground publication   (apps/zreview)
  │
  ▼
session::load   staged, blocking, no UI dependency        (crates/session)
```

`crates/session` owns `SessionRequest` and `load`. It has no GPUI dependency, so
the whole loading path is testable without a window, and whoever calls it decides
which thread it runs on.

`SessionView` is the state machine PLAN section 9 asks for. A stage report
arriving after the load finished is ignored, so a late update cannot drag a ready
or failed view back to loading.

`apps/zreview/src/loading.rs` is the only part that crosses threads. The blocking
load runs on the background executor and writes its stage — and finally its
result — into a shared slot; a foreground task wakes every 100ms to publish
whatever is there. A lock rather than a channel keeps this to the standard
library, because the foreground side only ever needs the *latest* stage, so there
is nothing to queue.

## Typed failures

`gh` exits 1 for every HTTP error and reports the status in stderr as
`gh: <message> (HTTP <status>)`, so the category comes from that rather than the
exit code: 401 unauthenticated, 403 forbidden or rate-limited depending on the
body, 404 not found, 422 validation, 5xx server, and a separate network case for
connection failures. Anything unrecognized stays a generic command failure instead
of being forced into a category that might be wrong.

Each category carries remediation where honest advice exists — `gh auth login`,
the `repo` scope and SSO, githubstatus.com. A rejected payload (422) deliberately
has none: that is a defect in this application, not something a reviewer can act
on. A missing `gh` executable is told apart from other spawn failures, because the
remediation is completely different.

Authentication is checked before the first API call, so an unauthenticated
reviewer is told to run `gh auth login` rather than shown a 401 from whichever
request happened to run first.

A conversation fetch that fails no longer prints to stderr and vanishes: it is
recorded on the session and shown in the sidebar, because a pull request that
silently appears to have no discussion is worse than one that says its discussion
is missing.

## Tests

- the loading glue end to end on GPUI's test executor, with the clock advanced
  deterministically: both a successful load and a failing one reach the window,
  so a broken poll loop cannot leave the reviewer on the loading screen;
- the state machine: loading → ready hands focus to the diff, loading → failed
  keeps summary, remediation and detail, and a late stage report cannot reopen a
  finished session;
- every `gh` failure category, its remediation expectation, and that the reported
  text survives classification;
- a missing `gh`, and an unauthenticated `gh` caught before any API call;
- stage ordering from both the GitHub client and the session loader;
- argument parsing, including the counts that are rejected.

Verified against `BurntSushi/ripgrep#3488`: all six stages reported in order, the
correct two files, and all three real review comments anchored to real diff rows
with nothing unplaced. Separately, an invalid `GH_TOKEN` now leaves the window
open with the error shown in-app and prints nothing to the terminal.

## Current limitations

- Loading cannot be cancelled, and there is no retry button on the failure
  screen — recovering means restarting the app.
- Stages are coarse. "Building the diff" covers one `git diff` per changed file,
  so it is the long pole on a large pull request with no finer feedback and no
  file count.
- Nothing is rendered until the whole session is ready. PLAN section 6 wants
  metadata and file summaries published first, then file contents lazily.
- The 100ms publication interval is a poll. It is invisible next to the
  subprocesses being waited on, but a channel would be tidier if progress ever
  needs finer granularity.
- There is still no `Refreshing` state; PLAN's session machine includes one.

## Next step

Local anchored drafts, on the anchor model from
[review-comments.md](review-comments.md): an inline composer that writes a draft
keyed to a validated anchor, a draft queue, and eager persistence. That is the
first piece of Phase 3 that needs a store, so it also forces the SQLite decision.
