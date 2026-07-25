# The comment editor

## Goal

Replace the prototype text field with something a review comment can actually be
written in.

## What it replaced

The old field appended characters and removed the last one. There was no caret, so
a typo in the middle of a comment could not be fixed without deleting everything
after it. There was no selection, no copy or cut, and no input-method support, so
any language needing composition was unusable.

## Shape

The editing rules live in `crates/ui/src/text.rs` as `TextBuffer`, which has no
GPUI dependency. Every decision about where the caret goes and what an edit
changes is made there and tested without a window; the view is left with key
dispatch, drawing, and the platform bridge.

Offsets are byte offsets that are always on grapheme boundaries, so a caret cannot
land inside a character and deleting once removes a whole cluster rather than
leaving a broken emoji. `unicode-segmentation` provides the boundaries.

## What works

- a caret that can be moved by character, line, and to the start or end of a line;
- selection by `⇧`-arrow and `⌘A`, replaced by whatever is typed next;
- backspace and delete, which remove the selection when there is one rather than a
  single character — which is what stops a reviewer losing the character they
  meant to replace;
- multi-line comments, with vertical movement that keeps its column and clamps to
  the end of a shorter line;
- cut, copy, and paste;
- input-method composition through `EntityInputHandler`, with the composed text
  visible in place while it is being chosen.

Composition counts as an edit, so an in-progress composition is stored like any
other draft text rather than being lost to a crash.

## The platform bridge

`EntityInputHandler` speaks UTF-16 offsets, because that is what macOS input
methods use, so the buffer converts in both directions. Offsets arriving from a
platform input method are snapped onto grapheme boundaries: they cannot be trusted
to be boundaries, and slicing a `String` off one panics.

The handler is installed by a zero-size `canvas` layered over the field, because
`Window::handle_input` must be called during paint and asserts as much. Getting
that wrong was the one real trap — installing it during *prepaint* compiles and
then panics every test that focuses the field.

## Tests

- the buffer, without a window: insertion at the caret, backspace at the start,
  deleting a selection, replacing a selection by typing, grapheme clusters moved
  and deleted as one unit, selection extending from the fixed end, line-relative
  home and end, vertical movement keeping its column and clamping to a short line,
  UTF-16 round trips, composition being replaced rather than appended, and offsets
  inside a character or past the end being snapped;
- through the view: fixing a typo mid-word, typing over a selection, `⌘A` then
  replacing everything, a comment spanning several lines, home and end within a
  line, cut and paste, and every one of those edits reaching the stored draft.

Typing in a test now routes through the platform input handler rather than a key
listener, so those tests exercise the same path a real keystroke takes.

## Current limitations

- **The caret cannot be positioned with the mouse.** Clicking focuses the field
  but does not move the caret, and there is no drag-selection. Both need
  shaped-line hit testing, which means a custom element that measures glyphs.
- For the same reason the caret is drawn between text spans rather than at a
  measured offset, so it is approximately placed within a line, and an
  input-method candidate window appears beside the field rather than at the caret.
- No word-wise movement (`⌥`-arrow), no undo, and no scrolling within the field: a
  long comment grows the row instead.
- No undo means `⌘A` followed by a keystroke is unrecoverable.

## Next step

A custom element that shapes each line would fix mouse positioning, exact caret
placement, and the candidate-window position together, since all three need the
same glyph metrics.
