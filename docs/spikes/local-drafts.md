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

A draft may cover a range of lines. `⇧J`/`⇧K` extends the selection and the
composer opens over it; the draft is keyed by the range's *last* line, which is
where GitHub anchors a range and where it is drawn — so widening an existing
comment into a range edits that comment rather than leaving a rival beside it. A
one-row span stays a single-line draft, because sending `start_line == line` would
have GitHub reject an otherwise valid comment.

A range must be submittable, not merely selectable. Both ends must anchor on the
same side, and both must sit in the same hunk: line numbers run contiguously within
a hunk on each side, so matching hunks is what guarantees every line between them
is in the diff too, which is GitHub's requirement. A span that fails either test is
refused rather than quietly truncated to something that would go through.

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

A stale draft can then be moved: select a line in the current diff and the panel
listing the draft offers to move it there. The move is validated like any other
draft creation, refuses a row that cannot carry a comment, and refuses a row that
already holds a draft rather than overwriting it. It reaches storage as a removal
*and* a write, because a move that only wrote would come back in both places.

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
  through a recording sink;
- re-anchoring: a stale draft moving onto a row and ceasing to be stale, refusal
  on a row that cannot carry a comment and on one that already holds a draft
  (neither consuming the text), and the move reaching storage as both a removal
  and a write.

Verified against a real repository: a draft on a deleted line anchored to
`LEFT` line 2, was written to SQLite with the expected scope, head, path, side and
line, and came back non-stale on reopen with no warnings.

## Current limitations

- A range cannot straddle both revisions. GitHub allows a separate `start_side`,
  but a comment whose start and end are on different sides is far easier to create
  by accident than to mean, so it is not offered.
- Re-anchoring is one draft at a time, from the file's panel, and only onto the
  currently selected row. There is no way to move several at once.
- ~~The composer is still the prototype editor.~~ Replaced; see
  [comment-editor.md](comment-editor.md).
- ~~Nothing is submitted anywhere yet.~~ See
  [review-submission.md](review-submission.md).
- A draft is written on every keystroke rather than on a short debounce as PLAN
  suggests. WAL makes it cheap enough that the simpler behaviour is safer, but a
  debounce would cut the write volume.
- Only drafts are stored. Viewed state, the review summary, snapshot metadata, and
  trust decisions are all still in memory, and PLAN wants them here.
- There is no **Clear review data** action, which PLAN requires alongside stored
  review material.
- Drafts from heads that no longer exist accumulate; nothing prunes them.

## Next step

The review summary and batch submission landed in
[review-submission.md](review-submission.md).
