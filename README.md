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
- `c`: open the inline comment editor on the selected line, loading any draft already there
- `esc`: dismiss the inline comment editor, keeping the draft
- `⌘C`: copy the selected line
- `⇧⌘J` / `⇧⌘K`: select the next or previous changed file
- `⇧⌘V`: toggle the selected file's viewed state
- Mouse: select files or diff rows and use the selected row's **Comment** button

## Validate

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo deny check
```

CI runs the first three on macOS and `cargo deny check` on Linux. `cargo deny` needs `cargo install cargo-deny` locally.

## Current scope

Local comparisons are loaded through the `git` executable without a shell, external diff drivers, or text-conversion filters. The parser supports multiple hunks, additions, deletions, renames, copies, binary detection, unusual UTF-8 names, missing-final-newline markers, and direct or merge-base comparison modes.

The UI includes a virtualized changed-file sidebar with status, line counts, keyboard navigation, in-memory viewed state, and pinned GitHub PR source metadata.

Existing GitHub review conversations are loaded and rendered read-only inside the diff. Replies are collapsed into threads and each thread is anchored to the line its opening comment sits on, using the same `path`/`side`/`line` model GitHub submission requires. A thread GitHub reports without a usable position — outdated, whole-file, or outside a displayed hunk — is listed against its file with the reason instead of being dropped. Replying to and resolving threads are deliberately out of scope for the MVP.

The window opens before any Git or GitHub work starts. Loading runs on a background executor and reports its stage, and a failure is shown in the app with the next action to take — `gh auth login` for an unauthenticated CLI, a link for a missing one, and so on — rather than printed to a terminal you may not be watching. Only argument errors are still reported on the command line.

Comments you write become local drafts anchored to the line they are on, saved as you type into a bundled SQLite database under `~/Library/Application Support/ZReview`. They come back when you reopen the same pull request, including drafts written before the branch was pushed to — those are kept and marked as needing re-anchoring rather than discarded.

It does not yet submit reviews, re-anchor a stale draft, persist anything besides drafts, provide syntax highlighting, or run an AI review backend. The comment field is a minimal keyboard-input prototype used to validate focus and variable-height virtualized rows; it will be replaced by an IME-aware production editor.

## License

Licensed under either Apache-2.0 or MIT, at your option.

`deny.toml` enforces that boundary in CI: every dependency licence must be on an explicit allow list, so a strong-copyleft crate — Zed's GPL-3.0 `ui` crate being the specific one to keep out — cannot arrive unnoticed. Zed's `gpui` itself is Apache-2.0 and fine to depend on.

One deliberate exception is recorded there: `option-ext` is MPL-2.0 and reaches the binary through `gpui`'s font discovery. MPL-2.0 is file-level copyleft that does not extend to code merely linking it, but it is not strictly permissive, and it is one of the items PLAN wants confirmed by legal review before distribution.
