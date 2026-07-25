# Diff anchors and existing review comments

## Goal

Give every position in a review one representation that GitHub will accept, and
use it to show a pull request's existing conversation inside the diff.

This is the second half of the golden-anchor spike, and the last piece of the
diff viewer before local drafts.

## Anchor model

`crates/domain` now owns `DiffAnchor`, `DiffSide`, and `AnchorIndex`.

An anchor is a `path`, a `side`, a 1-based `line` on that side, and the
`head_sha` it was created against. Anchors are single-line: PLAN sequences
multiline ranges after the single-line model is proven, so there is no
`start_line` field to leave unvalidated.

`AnchorIndex` maps both directions for exactly one snapshot:

- forwards, a displayed row becomes the anchor GitHub expects — additions exist
  only on the right, deletions only on the left, context lines are anchored right
  to match GitHub's own review UI, and rows with no source line cannot be
  commented on;
- backwards, `resolve` locates an anchor's displayed row, rejecting anchors from
  another head, paths that are not under review, and lines outside a displayed
  hunk.

`resolve` is the `AnchorValidator` PLAN section 6 requires. Nothing becomes an
inline comment without passing it, whether it came from a reviewer, a review
backend, or an earlier session.

`ReviewSession` builds the index when its source has a head commit, so a snapshot
and its anchors cannot drift apart. Demo sessions have no head and no index.

## Existing comments

`crates/github` fetches published comments with `gh api --paginate`. Pages are
parsed as concatenated JSON values, which reads both the merged single array
current `gh` versions emit and the one-array-per-page framing, so the installed
CLI cannot change what is loaded. A deleted author falls back to GitHub's own
`ghost` placeholder. An unrecognized diff side is reported rather than guessed,
because defaulting it would place the comment against the wrong revision.

`domain::PlacedComments` then decides where each conversation belongs:

- replies collapse into threads, following `in_reply_to_id` to the root with
  memoized lookups so a deep chain costs one step per comment;
- a reply naming a parent outside the response starts its own thread rather than
  vanishing;
- a thread's position comes from its opening comment, which is what GitHub does;
- row lookup is keyed by `(file, row)` so the virtualized list can ask per
  visible row without scanning.

A thread that cannot be anchored is kept and labelled, never dropped — an
outdated comment is still part of the conversation a reviewer needs to read. The
four reasons are distinguished, because calling a whole-file comment "outdated"
would be wrong: nothing about it is stale.

`crates/ui` renders anchored threads read-only beneath their row, reusing the
variable-height list rows the composer already proved. The sidebar and the
per-file panel show conversation counts, and the panel lists the file's unplaced
threads with their reason.

## Tests

- bidirectional round trip over every commentable row (PLAN section 11), side and
  kind agreement, the gap between two hunks, stale snapshots, unknown paths, and
  binary files;
- thread grouping: replies, replies to replies, replies whose parent is missing,
  reply cycles, stable ordering when several threads share a row, and position
  taken from the root rather than a reply;
- the four unplaced reasons, including whole-file separated from outdated;
- a golden fixture captured from the live API, carrying 26 fields per comment
  where the mapper reads nine;
- pagination framing in both shapes, absent and unknown sides, deleted authors,
  and multi-line ranges;
- an exact argv assertion covering `--paginate` and the pinned API version;
- a GPUI test rendering an anchored thread and an outdated one in a session, then
  navigating to the commented row and typing into the composer beside it.

The `gh api --paginate` argument array was also run against a live PR and
returned exactly the committed fixture.

## Current limitations

- ~~Comment fetching is another blocking call before the window opens.~~ Fixed by
  [async-session-loading.md](async-session-loading.md).
- Threads render at full body length; there is no collapse, and a very long
  conversation makes a tall row.
- Reactions, resolved state, review submission grouping, and pending reviews are
  not read. GitHub's REST comment list does not expose resolved state, so that
  needs the GraphQL API.
- Replying and resolving are out of MVP scope.
- Timestamps are shown raw rather than relative.

## Next step

Session loading moved off the blocking path in
[async-session-loading.md](async-session-loading.md), and the anchor model this
spike landed now carries local drafts — see [local-drafts.md](local-drafts.md).
