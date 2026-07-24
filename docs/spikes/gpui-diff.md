# GPUI diff virtualization spike

## Goal

Prove that permissively licensed GPUI can support ZReview's central interaction: a large, keyboard-navigable unified diff with an inline, variable-height comment composer.

## Implementation

- GPUI is pinned exactly to `0.2.2` with only the macOS `font-kit` runtime feature.
- `crates/domain` defines `DiffFile`, `DiffHunk`, `DiffLine`, and side-specific line coordinates.
- `crates/ui` renders the diff with GPUI's variable-height virtualized `list`.
- The app fixture contains 100,000 lines.
- Additions, deletions, context lines, two line-number gutters, selected state, and hunk metadata are rendered distinctly.
- Keyboard and mouse line selection are supported.
- The selected source line can be copied with Command-C.
- A focused comment composer can be inserted into a row without changing the list's item count.

The comment composer is intentionally minimal. It validates focus, typing, paste, newline input, and changing row height. It is not yet an IME-aware production editor.

## Automated coverage

The GPUI smoke test opens a window containing all 100,000 domain rows, navigates to another row, opens its comment composer, types into it, and verifies state.

```text
cargo test -p ui
1 passed; finished in 0.20s (warm debug build)
```

Domain tests validate generated line coordinates and row markers.

## Initial runtime baseline

Measured from the debug executable on Apple Silicon macOS after the window had been open for three seconds:

```text
resident memory: 113,456 KiB (~111 MiB)
```

The process remained active during the launch smoke test, and the initial window rendered the full virtualized fixture without materializing 100,000 GPUI row elements at once. Release-mode and scrolling-frame measurements should be added before setting a production performance budget.

## Outcome

The core virtualization approach is viable. Keep GPUI behind `crates/ui`, and continue using a flattened diff-line model for constant-time virtual row lookup.

Before calling the production diff viewer complete, replace the prototype composer, add true character-range source selection, add accessibility semantics, and profile sustained scrolling in a release build.

## Next step

Implement the local Git diff parser and golden fixtures in `crates/git`, then feed a real `DiffFile` into this viewer without adding GitHub networking yet.
