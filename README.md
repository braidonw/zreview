# ZReview

A native macOS GitHub pull-request review app built with Zed's GPUI framework.

The current implementation is the first risk-reduction spike: a virtualized, keyboard-navigable 100,000-line unified diff with an inline comment editor.

## Run

Requirements: macOS, Xcode command-line tools, and the pinned Rust toolchain.

```bash
cargo run -p zreview
```

Controls:

- `j` / `↓`: select the next line
- `k` / `↑`: select the previous line
- `c`: open or close the inline comment editor
- `⌘C`: copy the selected line
- Mouse: select a row and use its **Comment** button

## Validate

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Scope of this spike

This version uses generated diff data. It intentionally does not include Git/GitHub access, persistence, syntax highlighting, or an AI review backend. The comment field is a minimal keyboard-input prototype used to validate focus and variable-height virtualized rows; it will be replaced by an IME-aware production editor.

## License

Licensed under either Apache-2.0 or MIT, at your option.
