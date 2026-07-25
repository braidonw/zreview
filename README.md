# ZReview

A native macOS GitHub pull-request review app built with Zed's GPUI framework.

The current implementation includes a virtualized, keyboard-navigable unified diff, an inline comment editor, local Git comparisons, and GitHub PR loading through `gh`.

## Run

Requirements: macOS, Xcode command-line tools, the pinned Rust toolchain, and an authenticated GitHub CLI (`gh auth login`) for PR loading.

Run the generated 100,000-line fixture:

```bash
cargo run -p zreview
```

Or review every changed file in a real local comparison. The head defaults to `HEAD`:

```bash
cargo run -p zreview -- /path/to/repository main
cargo run -p zreview -- /path/to/repository base-branch feature-branch
```

The comparison uses merge-base semantics, equivalent to `base...head`.

Load a GitHub PR using the current repository, or provide a local clone explicitly:

```bash
cargo run -p zreview -- pr 123
cargo run -p zreview -- pr /path/to/repository 123
cargo run -p zreview -- pr /path/to/repository https://github.com/acme/widgets/pull/123
```

ZReview reads metadata with `gh api`, fetches the base branch and `refs/pull/<number>/head` into `refs/zreview/...`, verifies the fetched head against the API response, and then renders the local merge-base comparison. User branches and `FETCH_HEAD` are not changed.

The comparison is taken against the merge base of the current base branch tip and the head, which is what GitHub's own "Files changed" view shows. GitHub's recorded `base.sha` is pinned when a PR is created or synchronized and drifts as the base branch advances, so it is kept as provenance only and never defines the comparison.

Controls:

- `j` / `↓`: select the next line
- `k` / `↑`: select the previous line
- `c`: open the inline comment editor
- `esc`: dismiss the inline comment editor
- `⌘C`: copy the selected line
- `⇧⌘J` / `⇧⌘K`: select the next or previous changed file
- `⇧⌘V`: toggle the selected file's viewed state
- Mouse: select files or diff rows and use the selected row's **Comment** button

## Validate

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Current scope

Local comparisons are loaded through the `git` executable without a shell, external diff drivers, or text-conversion filters. The parser supports multiple hunks, additions, deletions, renames, copies, binary detection, unusual UTF-8 names, missing-final-newline markers, and direct or merge-base comparison modes.

The UI includes a virtualized changed-file sidebar with status, line counts, keyboard navigation, in-memory viewed state, and pinned GitHub PR source metadata.

Existing GitHub review conversations are loaded and rendered read-only inside the diff. Replies are collapsed into threads and each thread is anchored to the line its opening comment sits on, using the same `path`/`side`/`line` model GitHub submission requires. A thread GitHub reports without a usable position — outdated, whole-file, or outside a displayed hunk — is listed against its file with the reason instead of being dropped. Replying to and resolving threads are deliberately out of scope for the MVP.

It does not yet submit reviews, persist state, provide syntax highlighting, or run an AI review backend. The comment field is a minimal keyboard-input prototype used to validate focus and variable-height virtualized rows; it will be replaced by an IME-aware production editor.

## License

Licensed under either Apache-2.0 or MIT, at your option.
