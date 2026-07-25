# Review summary and batch submission

## Goal

Turn a session full of local drafts into one submitted GitHub review, without
ever posting something a human has not seen.

This completes Phase 3.

## The rule everything is arranged around

PLAN states that the application must never post a comment without an explicit
human submission action. That is not a UI detail, so it is enforced structurally:
assembling a review and sending one are separate operations with a person between
them.

`ReviewSession::prepare_submission` builds the exact request and touches no
network. `ReviewSubmitter::submit` sends a request it is given and builds nothing.
The confirmation panel holds the assembled request itself, so what the reviewer
approves is what leaves the machine rather than a re-derivation of it.

A test asserts the negative directly: requesting submission posts nothing.

## What the anchor model was for

Every draft is re-resolved against the snapshot as it is added to the submission,
so a position GitHub would reject cannot reach the request even if it was valid
when the draft was written. Drafts that no longer resolve are returned as
`excluded` and shown in the confirmation under "will NOT be posted", because a
reviewer told nothing would believe they had submitted them.

## Matching what GitHub accepts

Checked against the REST documentation rather than assumed:

- `body` is required for `COMMENT` and `REQUEST_CHANGES`, optional for `APPROVE`.
  An approval with no body omits the key entirely — an empty string is not the
  same as absent.
- `event` is always sent. Omitting it creates a *pending* review, which PLAN rules
  out: it conflicts with an existing pending review and moves crash recovery
  outside this application's control.
- `line` and `side` are used, not the closing-down `position` parameter. A test
  asserts `position` never appears in a payload.
- An empty review — no comments and no summary — is refused before it is sent.

`commit_id` pins the review to the snapshot's head, so the forge independently
rejects it if the head moved.

## Not losing a review to a failure

The head is re-read and the submission refused if the pull request advanced, so a
review cannot attach comments to code the author has already replaced.

Nothing local is touched by sending. Drafts and the summary are forgotten only
after the forge returns success, and only the anchors that were actually posted —
excluded drafts are left alone, because they were not sent and are still the only
copy. Storage is told what was consumed in one transaction, so a crash cannot
leave the summary behind without its comments.

A failed submission reports the failure leading with the fact that the drafts are
unchanged, because that is what a reviewer needs to know first.

The payload goes to `gh` over stdin rather than as an argument, so a comment body
cannot appear in a process listing and its size is not bounded by the argument
limit.

## The summary

Stored in its own table keyed by head, because it belongs to the review rather
than to any line, and saved as it is typed like any draft. Schema version 2.

It reuses the inline composer, which means it inherits the keybinding isolation
that editor needed — and its limitations.

## Tests

- requesting submission posts nothing; cancelling posts nothing and keeps the
  summary;
- confirming posts exactly once, then clears the session and tells storage which
  anchors were consumed;
- a failed submission keeps every draft and the summary, and clears nothing;
- a review GitHub would reject is refused before sending, with the reason;
- payload shape: pinned head, `line`/`side`, no `position`, an approval omitting
  `body`, a summary-only review omitting `comments`;
- the stale-head refusal, asserting nothing was posted;
- the payload reaching `gh` on stdin, through a fake that captures it;
- the domain rules: what is included, what is excluded, every refusal, and that
  marking submitted forgets only what was posted.

## Current limitations

- **Nothing has been submitted to a real pull request.** Every layer is tested
  against fakes and the payload is checked against the documented schema, but the
  first real `POST` has not happened. That is the one thing left to verify.
- No multiline comment ranges, so a draft is always a single line. GitHub's
  `start_line`/`start_side` are unused.
- Submission cannot be cancelled once sending starts.
- A submitted review does not appear in the diff as a published thread until the
  session is reopened.
- Approving with unaddressed stale drafts is allowed, with the confirmation
  listing them. Whether that should be a harder stop is a product question.
- The summary uses the prototype editor: append and backspace only.

## Next step

Submit to a real pull request and confirm the response, then Phase 4 — guidance
discovery and the first review backend. The backend choice in PLAN section 13 is
still open.
