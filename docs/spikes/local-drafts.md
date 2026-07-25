# Local anchored drafts

## Goal

Let a reviewer write comments that are pinned to a validated position and that
survive the process. This is the first half of Phase 3, and the first state worth
persisting.

## What it replaced

The composer was a prototype whose text went nowhere. Closing it discarded what
had been written; so did switching files. Nothing was ever anchored, so nothing
could have been submitted.

## Anchoring

A draft is text plus the [anchor](review-comments.md) it will be submitted
against, and the anchor is validated when the draft is created — so a draft that
exists is one GitHub would accept. A row that cannot carry a comment is refused
outright rather than accepting text that could never be posted.

One anchor holds one draft. Reopening the composer on a line loads what is there
and continues it, instead of silently starting a second comment over the top of
the first. The two sides of a context line are separate positions and hold
separate drafts. An emptied composer removes the draft rather than storing a blank
one.

Drafts are ordered by path then line: the order a reviewer reads, and the order a
submission should list them in.

## Never losing text

A draft restored from an earlier session may no longer resolve. The diff genuinely
changes underneath drafts — a base branch that moves shifts the merge base, and
with it which lines are displayed — and a pull request can be pushed to
mid-review.

Every layer refuses to drop text in that situation:

- the persisted scope is the pull request's repository and number, *not* its head,
  so a push does not orphan what was written before it;
- `DraftStore::load` returns drafts from every head it holds, leaving the decision
  to the session;
- `restore_drafts` keeps anything that will not resolve, marked stale, and reports
  how many;
- the loader turns that count into a warning that says the drafts are kept and
  what has to happen before they can be submitted;
- the diff marks a stale draft as needing re-anchoring rather than hiding it.

Re-anchoring itself is not implemented yet — see the limitations.

## Persistence

`crates/store` owns a bundled SQLite database under
`~/Library/Application Support/ZReview`. SQLite is compiled in rather than linked
against the system library, so the schema the app was tested against is the schema
it ships with. `PRAGMA user_version` carries the schema version, so there is no
bookkeeping table.

Drafts are keyed by `(scope, head_sha, path, side, line)`. WAL journalling with
`synchronous = NORMAL` makes a per-keystroke write cheap — no fsync per commit —
so a crash can lose at most a fraction of a second rather than corrupting the
file.

`DraftWriter` owns a thread and a channel, because PLAN's performance budget rules
out database work on the UI thread and a draft is written on every keystroke.
Dropping it finishes the queue before returning, so quitting does not drop the
last few characters. Write failures are recorded rather than returned — there is
no useful answer to give a keystroke — and surface as a banner above the diff,
because writes failing means work is being lost as it is typed.

`DraftSink` is declared in `domain`, next to the drafts themselves: the store
implements it and the UI consumes it, so `crates/ui` still depends only on the
domain and knows nothing about a database. Where the database lives is passed into
`session::load` as `DraftStorage`, which keeps tests off the reviewer's real
review data and makes relocating it later a matter of configuration.

## Tests

- typing produces an anchored draft, survives closing the composer and changing
  files, reopening loads it, and emptying removes it;
- rows that cannot carry a comment are refused; a session with no head holds no
  drafts;
- the store: round trip, upsert replacing a body, both sides of a line, scope
  isolation, delete, reading order, surviving a reopen, migration not re-running,
  and an unreadable stored side being reported;
- the writer: persisting and removing through the sink, and the last of a burst of
  per-keystroke writes winning;
- the session loader: a draft surviving a reopen, a discarded one staying gone, a
  draft from another head restored as stale *with* its warning, and unusable
  storage warning while still opening the session;
- the last link, that a keystroke reaches the sink and a discard reaches it too,
  through a recording sink.

Verified against a real repository: a draft on a deleted line anchored to
`LEFT` line 2, was written to SQLite with the expected scope, head, path, side and
line, and came back non-stale on reopen with no warnings.

## Current limitations

- **A stale draft cannot be re-anchored.** It is kept, shown, and counted, but
  moving it to a line in the current diff has no UI yet. This is the most
  important gap.
- The composer is still the prototype editor: append and backspace, no cursor, no
  selection, no IME. Editing a long draft is genuinely awkward.
- Nothing is submitted anywhere yet. Drafts accumulate locally.
- A draft is written on every keystroke rather than on a short debounce as PLAN
  suggests. WAL makes it cheap enough that the simpler behaviour is safer, but a
  debounce would cut the write volume.
- Only drafts are stored. Viewed state, the review summary, snapshot metadata, and
  trust decisions are all still in memory, and PLAN wants them here.
- There is no **Clear review data** action, which PLAN requires alongside stored
  review material.
- Drafts from heads that no longer exist accumulate; nothing prunes them.

## Next step

Re-anchoring stale drafts, then the review summary and batch submission — the rest
of Phase 3. Submission is where the anchor validator earns its place: only anchors
that resolve may become inline comments, and everything else has to be moved or
folded into the summary.
