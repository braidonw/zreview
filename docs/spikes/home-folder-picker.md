# Home folder picker and the repositories list

## Goal

Establish what the Home screen's Repositories panel can rely on before it is built. The panel adds a local clone through the native macOS folder picker and writes the chosen paths to `~/.config/zreview/settings.toml` as a plain list. Five questions were put to primary sources. The GPUI API for a directory-only open panel and how it is awaited from a view. Whether the panel works without entitlements from `cargo run` and from a `.app` bundle. Whether writing under `~/.config` has sandbox consequences given the distribution plan. Whether `toml` 1.x is enough for a list of paths or `toml_edit` is warranted. How to resolve `~/.config`.

Everything below was read from the pinned GPUI source in the local cargo registry (`gpui` `=0.2.2`, `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-0.2.2`, pinned in `Cargo.toml` and resolved in `Cargo.lock` line 2135), from the resolved `toml` `1.1.3+spec-1.1.0` and `serde` `1.0.229` sources in the same registry, from Apple's developer documentation, and from Zed's own callers of the same API. Losing hand formatting and comments on rewrite is an accepted decision and is only confirmed here, not reopened.

## 1. The GPUI API

`App::prompt_for_paths(&self, options: PathPromptOptions) -> oneshot::Receiver<Result<Option<Vec<PathBuf>>>>` is the whole surface (`gpui-0.2.2/src/app.rs:1116-1121`). Its doc comment says the paths are relayed asynchronously through the returned oneshot channel, that a cancelled panel relays `None`, and that the error arm is for Linux when the picker cannot be opened (`app.rs:1111-1115`). `oneshot` is `futures::channel::oneshot` and `Result` is `anyhow::Result` (`app.rs:13-17`; `futures` is pinned at `0.3` in `gpui-0.2.2/Cargo.toml:270-271`).

`PathPromptOptions` has four public fields, `files: bool`, `directories: bool`, `multiple: bool`, and `prompt: Option<SharedString>` (`gpui-0.2.2/src/platform.rs:1328-1339`). A directory-only picker is `PathPromptOptions { files: false, directories: true, multiple: true, prompt: ... }`. `prompt` sets the panel's confirm button title through `setPrompt:` (`gpui-0.2.2/src/platform/mac/platform.rs:748-750`).

The macOS implementation (`mac/platform.rs:710-758`) creates an `NSOpenPanel`, applies `setCanChooseDirectories_`, `setCanChooseFiles_`, and `setAllowsMultipleSelection_` straight from the three booleans, additionally sets `setCanCreateDirectories(true)` and `setResolvesAliases_(false)`, and calls `beginWithCompletionHandler:` with a block. On `NSModalResponseOk` the block walks `panel.URLs()`, keeps every file URL that converts through `ns_url_to_path`, and sends `Ok(Some(paths))`. Any other response sends `Ok(None)`. The panel is created on the foreground executor and the whole thing is detached, so the receiver is the only handle the caller gets. `ns_url_to_path` (`mac/platform.rs:1539-1547`) builds the `PathBuf` from `fileSystemRepresentation` via `OsStr::from_bytes`, so what comes back is a byte path and not a lossy string, and the panel is non-modal (a sheetless `begin`, not `runModal`), so the app keeps rendering while it is open.

Awaiting the receiver adds a third layer. `futures::channel::oneshot::Receiver<T>` is a `Future` with `Output = Result<T, Canceled>` (`futures-channel-0.3.33/src/oneshot.rs:455-456`), so the awaited value is `Result<anyhow::Result<Option<Vec<PathBuf>>>, Canceled>`. `Canceled` cannot happen on macOS in practice because the sender lives in the completion block until the panel closes, but the type still has to be unwrapped.

From a view the call goes through `Context<V>`, which derefs to `App` (`gpui-0.2.2/src/app/context.rs:25-31`), so `cx.prompt_for_paths(...)` works directly in an event handler. The receiver is then awaited inside `Context::spawn`, whose closure signature is `AsyncFnOnce(WeakEntity<T>, &mut AsyncApp) -> R` (`context.rs:237-245`), or `Context::spawn_in(window, ...)` which additionally provides an `AsyncWindowContext` (`context.rs:661-668`). `WeakEntity::update` returns `Result<R>` and fails only if the entity has been released (`gpui-0.2.2/src/app/entity_map.rs:689-699`), which is the existing pattern in `crates/ui/src/lib.rs:2558-2589`.

The pinned crate ships no example or test that calls `prompt_for_paths`. The test platform's implementation is `unimplemented!()` (`gpui-0.2.2/src/platform/test/platform.rs:334-339`), so a test that reaches the picker will panic under `TestAppContext`, and the code path that opens the panel should stay out of headless tests. Zed's `git_ui` crate is the closest first-party caller and it is exactly this shape. From `crates/git_ui/src/clone.rs` at commit `f8c27835` on `main` (https://github.com/zed-industries/zed/blob/f8c278352fcc4289de66db79b599c98c1a57d351/crates/git_ui/src/clone.rs#L17-L27):

```rust
let destination_prompt = cx.prompt_for_paths(gpui::PathPromptOptions {
    files: false,
    directories: true,
    multiple: false,
    prompt: Some("Select as Repository Destination".into()),
});

window
    .spawn(cx, async move |cx| {
        let mut paths = destination_prompt.await.ok()?.ok()??;
        let mut destination_dir = paths.pop()?;
        ...
```

The `.ok()?.ok()??` chain is the three layers in order. Channel cancellation, the platform error, and the user cancelling all end the task. Zed's workspace picker (`crates/workspace/src/workspace.rs`, `prompt_for_open_path`, same commit) does the same but surfaces the `Err` arm as a visible error before falling back to its own picker, which is only relevant on Linux.

Translated to this codebase, where views spawn through `cx.spawn(async move |this, cx| ...)` and publish results with `this.update(cx, ...)`, the shape for the Repositories panel is:

```rust
fn add_repository(&mut self, cx: &mut Context<Self>) {
    let picked = cx.prompt_for_paths(PathPromptOptions {
        files: false,
        directories: true,
        multiple: true,
        prompt: Some("Add".into()),
    });
    cx.spawn(async move |this, cx| {
        let Ok(Ok(Some(paths))) = picked.await else {
            return;
        };
        this.update(cx, |this, cx| this.repositories_added(paths, cx)).ok();
    })
    .detach();
}
```

`picked` must be created before the `spawn` because `prompt_for_paths` needs `&App` and the async closure only has `&mut AsyncApp`. The `let else` collapses the three failure arms into one silent return, which matches the semantics (nothing was chosen). On macOS the `Err` arm is unreachable per the implementation above, so there is nothing to report in it.

## 2. Does the panel need entitlements

No. Apple's `NSOpenPanel` reference says the system draws Open panels in a separate process in macOS 10.15 and later regardless of whether the app is sandboxed, and that when the user chooses a file macOS adds it to the app's sandbox (https://developer.apple.com/documentation/appkit/nsopenpanel). The separate-process drawing and the sandbox extension are what the App Sandbox documentation calls Powerbox behaviour, and they are a benefit granted to sandboxed apps, not a requirement placed on unsandboxed ones.

The only entitlement that mentions open panels is `com.apple.security.files.user-selected.read-write`, and its documentation says it is added by enabling the App Sandbox capability in Xcode and setting User Selected File to Read/Write (https://developer.apple.com/documentation/bundleresources/entitlements/com.apple.security.files.user-selected.read-write). It widens a sandbox. An app without `com.apple.security.app-sandbox` has no sandbox to widen and reads any path the POSIX permissions allow.

The Hardened Runtime, which notarisation does require ("To upload a macOS app to be notarized, you must enable the Hardened Runtime capability"), "doesn't affect the operation of most apps, but it does disallow certain less common capabilities, like just-in-time (JIT) compilation". Its exception entitlements cover runtime protections (JIT, unsigned executable memory, library validation, debugging) and resource access (camera, microphone, location, address book, calendars, photos, Apple Events). None of them concern file dialogs or file system paths (https://developer.apple.com/documentation/security/hardened-runtime).

So the picker works from `cargo run`, which produces an unsigned, unbundled binary, and it will keep working from a signed and notarised `.app` with the Hardened Runtime, as long as the App Sandbox entitlement is not added. The binary from `cargo run` is not a bundle, so the panel is hosted by a process with no `Info.plist`, but GPUI's `App` already runs an `NSApplication` for the window and the panel attaches to the same one. There is no bundle tooling in the repository yet (no `Info.plist`, no `.entitlements`, nothing in the README), so the bundle claim rests on the Apple documentation rather than a local run.

## 3. Writing under `~/.config` and the distribution plan

PLAN.md does not plan the App Sandbox. Phase 5 lists "macOS signing, notarization, application bundle, and `zreview` terminal helper" (`PLAN.md:390-397`) and section 12's risk table (`PLAN.md:435-446`) has no sandbox row. Nothing else in the repository mentions sandboxing or entitlements. Signing and notarisation need the Hardened Runtime, not the App Sandbox. Apple states App Sandbox is mandatory only for the Mac App Store ("To distribute a macOS app through the Mac App Store, you must enable the App Sandbox capability", https://developer.apple.com/documentation/security/app-sandbox), and PLAN's distribution is direct.

So writing `~/.config/zreview/settings.toml` carries no entitlement consequence today, and the existing store already relies on the same freedom by writing `~/Library/Application Support/ZReview/review-data.sqlite3` with a hand-built path (`crates/store/src/lib.rs:646-651`).

For the record, if the App Sandbox were ever adopted the picture changes for both locations, not just `~/.config`. A sandboxed app gets "a container directory when launching your sandboxed app, to which the app has unrestricted read and write access", and "doesn't have unrestricted access to the user's home folder" (https://developer.apple.com/documentation/security/protecting-user-data-with-app-sandbox). The container is in `~/Library/Containers` (https://developer.apple.com/documentation/security/accessing-files-from-the-macos-app-sandbox), so `~/.config` and `~/Library/Application Support/ZReview` would both be outside it and both writes would fail. The picked repository paths would also stop being enough on their own, since a sandboxed app keeps access to a user-selected folder across launches only through a security-scoped bookmark, not a path (same page, the "Maintain access to files when your app next runs" section). Also worth knowing is that `dirs::config_dir()` on macOS returns `~/Library/Application Support`, not `~/.config` (`dirs-5.0.1/src/mac.rs:7-10` and the table in `dirs-5.0.1/src/lib.rs:96`). None of this blocks the current decision. It is a note against a future sandbox decision, which would have to migrate all local state at once.

## 4. `toml` versus `toml_edit`

`toml` 1.x is sufficient. The crate is already resolved at `1.1.3+spec-1.1.0` for `crates/review` (`Cargo.lock:5653-5654`, reverse dependency confirmed with `cargo tree -i toml@1.1.3`), and its default features include `serde`, `parse`, and `display` (`toml-1.1.3+spec-1.1.0/Cargo.toml:98-104`), so `toml::to_string`, `toml::to_string_pretty`, and `toml::from_str` are all available without touching the manifest (`toml-1.1.3+spec-1.1.0/src/ser/mod.rs:65,85` and `src/de/mod.rs:72`). Serialisation goes through `toml_writer` in 1.x (`Cargo.toml:104`), not `toml_edit`. The `toml_edit` already in the lock file (`0.22.27`) is pulled in only by `cbindgen`'s build dependency through `toml 0.8`, so choosing it would add a second, newer copy.

Run against the resolved crate, `#[derive(Serialize, Deserialize, Default)] #[serde(default)] struct Settings { repositories: Vec<PathBuf> }` behaves as follows.

- `to_string` writes one line, `repositories = ["/Users/braidon/Developer/zreview", '/tmp/with "quote" and \ backslash']`. A value containing a double quote is emitted as a literal single-quoted string, so backslashes are not doubled.
- `to_string_pretty` writes the array one element per line with four-space indentation and a trailing comma. This is the form to use, because adding a repository then produces a one-line diff in the file.
- An empty list writes `repositories = []`, an empty file reads back as an empty list under `#[serde(default)]`, and the round trip is exact.
- Comments and layout in an existing file do not survive a parse and rewrite. `# hello\nrepositories = [\n  "/a", # first\n]\n` came back as `repositories = ["/a"]\n`. That is the accepted loss. Nothing in the format or the crate forces the other way. `toml_edit`'s stated reason to exist is "parsing and modify toml documents, while preserving comments, spaces and relative order of items" (https://docs.rs/toml_edit/0.25.13/toml_edit/), and `toml`'s own `to_string_pretty` doc points at `toml_edit::DocumentMut` only "for greater customization" (`ser/mod.rs:81-83`). Neither is needed for a list the app owns.

`PathBuf` serialises through serde's blanket impl, which calls `Path::to_str` and returns `Error::custom("path contains invalid UTF-8 characters")` when it fails (`serde-1.0.229/src/core/ser/impls.rs:908-929`). TOML strings are UTF-8 by specification, so there is no encoding that could carry such a path. The scratch run confirmed `to_string` returns `Err(Custom("path contains invalid UTF-8 characters"))` for a path containing `0xFF`. `ns_url_to_path` returns raw bytes, so a non-UTF-8 folder name is representable on the picker side but not on the settings side. The realistic answer is to refuse the addition with a message rather than to poison the whole file, because a `to_string` failure on one path fails the serialisation of the entire struct. On macOS the file system normalises names to UTF-8 (APFS and HFS+ both store UTF-8), so this is a guard against a mount from elsewhere, not a case the panel will produce from a local disk.

## 5. Resolving `~/.config`

Match `crates/store/src/lib.rs:646-651`. It reads `HOME` with `std::env::var_os`, fails with `StoreError::NoHomeDirectory` when it is unset (`lib.rs:64-65`), and joins the rest by hand. The settings path should do the same, `PathBuf::from(home).join(".config/zreview/settings.toml")`, with the same error shape, and there is no reason for a new dependency.

Reasons not to pull in a crate for this.

- `dirs::config_dir()` returns `~/Library/Application Support` on macOS (`dirs-5.0.1/src/mac.rs:10`), which is the wrong answer for a file that is deliberately at `~/.config`. `dirs` is in the lock file only through `zed-font-kit`, and `directories` is not present at all.
- `std::env::home_dir` is stable and no longer carries a deprecation attribute in the pinned toolchain (`rustc 1.96.0`, `library/std/src/env.rs:641-644`). On Unix it "returns the value of the `HOME` environment variable if it is set (and not an empty string)" and falls back to `getpwuid_r` (`env.rs:609-615`). The `home` crate on Unix just calls it (`home-0.5.12/src/lib.rs:72-76`). The fallback matters only when `HOME` is absent, which for a GUI app launched by launchd or a terminal never happens, and the store already treats that as an error. Using `home_dir` instead of `var_os("HOME")` is defensible but would leave the two paths resolved by different rules, and consistency with the store is worth more here.

One knob that the spec supports but the ticket did not ask for is `XDG_CONFIG_HOME`, which the XDG Base Directory specification defines as the base for user configuration with the default "`$HOME`/.config" when unset or empty, and which must be absolute or be ignored (http://specifications.freedesktop.org/basedir/latest/). Honouring it is three lines and lets a user with a relocated `~/.config` keep the file with their other dotfiles. It is a nice-to-have. Left out, the app still lands in the spec's default location. If it goes in, the empty-string and relative-path rules above are the whole contract.

## What this settles

- Use `cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: true, prompt })` from the panel's handler, await the receiver in `cx.spawn`, treat `Ok(Ok(Some(paths)))` as the only success, and keep the panel out of headless tests because the test platform panics on it.
- No entitlement work now or at the Phase 5 bundle. The Hardened Runtime is needed for notarisation and does not touch this. The App Sandbox is not planned and would break the existing Application Support store as much as `~/.config`.
- `toml::to_string_pretty` over a `Settings { repositories: Vec<PathBuf> }` with `#[serde(default)]` is enough. Refuse a non-UTF-8 path at add time rather than letting it fail the whole write.
- Resolve the path from `HOME` exactly as `default_database_path` does, with the same missing-home error.
