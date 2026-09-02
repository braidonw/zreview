# ZReview

A native macOS app for reviewing GitHub pull requests against a repository's own guidance, then submitting one human-controlled review to GitHub.

## Language

**Home**:
The screen ZReview opens on when no pull request has been named. It lists pull requests and what each one wants from the user.
_Avoid_: Homepage, dashboard, inbox, board, landing page

**Session**:
One pull request or local comparison opened for review, pinned to an exact base and head.
_Avoid_: Tab, document, workspace

**Draft**:
A comment the user has written locally and not yet submitted, anchored to a line or range.
_Avoid_: Pending comment, unsent comment

**Finding**:
A suggestion proposed by a review backend. It becomes a Draft only when the user accepts it.
_Avoid_: AI comment, suggestion, issue

**Guidance**:
The review conventions a repository carries in its own files, discovered read-only when a Session opens.
_Avoid_: Rules, instructions, config
