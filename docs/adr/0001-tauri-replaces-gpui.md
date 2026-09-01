# Tauri replaces GPUI

The GPUI spike proved the virtualized diff was viable, but GPUI kept forcing the app to build framework rather than product. A text editor had to be written by hand, there is no accessibility tree on macOS, the app shell (bundling, updating) would be ours to build, and every UI change is a Rust recompile against a pinned 0.x API. We decided to rebuild the UI on Tauri 2 with a React, TypeScript, Vite, and plain CSS frontend, keeping all session state and the submission invariant in Rust behind thin commands. Native GPU rendering was incidental to the original choice, and a webview gives text input, IME, accessibility, and fast iteration for free.

## Considered options

- Stay on GPUI. Rejected because both drivers (hand-built framework pieces, slow UI iteration) compound as the app grows.
- egui. Rejected because it fixes neither driver fully. TextEdit and AccessKit cover text and accessibility, but iteration is still a Rust recompile and the result looks less native than GPUI.
- Dioxus. Same WKWebView as Tauri with one language, but the two hardest widgets (virtualized diff, comment editor) would stay hand-built, and the framework is a fast-moving 0.x. Rejected.

## Consequences

- The Rust crates (domain, git, github, review, session, store) are untouched. Orchestration moves from the GPUI views into a framework-free crates/app; Tauri commands are adapters over it.
- The migration runs side by side to exact current parity, verified against the running GPUI app, then deletes crates/ui and the GPUI dependency. Home (issue #2) is built after parity, in the new stack.
- True end-to-end tests are unavailable on macOS (tauri-driver has no macOS support). Invariants are tested in Rust on crates/app, frontend behavior in Vitest with the IPC boundary mocked, and generated TypeScript bindings (tauri-specta) turn contract drift into type errors.
