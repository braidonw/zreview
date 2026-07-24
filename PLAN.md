# ZReview product and implementation plan

## 1. Product goal

ZReview is a fast, native desktop application for reviewing a GitHub pull request against local project guidance before submitting a human-controlled review to GitHub.

The core loop is:

1. Open a PR from a local clone.
2. Snapshot the PR at an exact base and head SHA.
3. Read the diff, existing conversations, and repository review guidance.
4. Run a local/configured review engine.
5. Let the reviewer inspect, edit, accept, or dismiss every finding.
6. Add manual inline comments.
7. Submit one GitHub review as **Comment**, **Approve**, or **Request changes**.

The application must never post an AI-generated comment without an explicit human submission action.

## 2. Product decisions and MVP boundary

### Confirmed decisions

- ZReview will use a permissive license. Prefer dual MIT/Apache-2.0, use permissively licensed dependencies, and exclude Zed's GPL higher-level crates.
- The product is macOS-only; cross-platform support is not an MVP or architectural requirement.
- GitHub access, authentication, and review submission will use the installed `gh` CLI.
- Review content may be sent to a hosted model, with clear backend disclosure and no secret persistence.
- Repository review guidance will be discovered automatically, while remaining visible and overridable in the UI.

### Included

- macOS only.
- GitHub.com pull requests.
- An existing local Git clone whose remote corresponds to the PR.
- Authentication through an already authenticated `gh` CLI.
- PR metadata and existing inline review comments.
- A virtualized unified diff, with syntax highlighting, file navigation, viewed state, and inline comment composers.
- Single-line comments on the left or right side of the diff. Add multiline ranges after the basic anchor model is proven.
- Local draft persistence and recovery.
- One configurable review backend behind a backend-neutral interface.
- Automatic, transparent review-guidance discovery and optional trusted check commands.
- Batched review submission with `COMMENT`, `APPROVE`, and `REQUEST_CHANGES`.

### Deliberately deferred

- GitLab, Bitbucket, and GitHub Enterprise Server.
- Cloning repositories from inside the app.
- Split diff, image diff, commit-by-commit mode, and local working-tree review.
- Replying to or resolving existing GitHub threads.
- Editing PR metadata, merging, rebasing, or checking out the PR branch.
- Multiple simultaneous AI providers.
- Organization policy management or a hosted service.

This boundary produces the differentiating workflow without first rebuilding all of Codiff or Zed.

## 3. UI framework decision

Use **GPUI**, the GPU-accelerated Rust UI framework developed for Zed:

- `gpui` and `gpui_platform` provide windows, rendering, actions/keymaps, async integration, and test support.
- Pin GPUI to an exact crate version or Git revision. It is pre-1.0 and its own README warns that breaking changes are common.
- Build a small ZReview component layer for the app-specific shell and diff UI.
- Consider Apache-2.0 [`gpui-component`](https://github.com/longbridge/gpui-component) for commodity controls such as buttons, dialogs, menus, inputs, and themes. It is built on GPUI but is not Zed's own component crate.

### Licensing constraint

Zed's core `gpui` crate is Apache-2.0, but Zed's `ui` crate currently declares GPL-3.0-or-later. Because ZReview will be permissively licensed, importing or copying Zed's editor, multibuffer, git UI, or `ui` crates is out of scope. Use GPUI itself and app-owned or permissively licensed components, and enforce the boundary with dependency/license checks in CI. Confirm the final dependency set with legal review before distribution.

## 4. User experience

### Opening a PR

Support both:

```text
zreview https://github.com/acme/widgets/pull/123
zreview pr 123                       # uses the current repository
```

The desktop app also has an **Open Repository** action followed by a PR URL/number picker. On open, it checks `git`, `gh`, repository identity, authentication, and object availability, and shows actionable remediation instead of a generic failure.

### Main window

```text
┌ PR title, author, base ← head, SHA, checks, refresh ───────────────┐
│ Files / filters       │ Unified diff             │ Review queue   │
│                       │                          │                │
│ ✓ src/model.rs     2  │ @@ -20,5 +20,8 @@        │ Guidance       │
│   src/view.rs      1  │  context                 │ Run progress   │
│   tests/model.rs      │ +added line   [comment]  │ Findings       │
│                       │ -deleted line             │ Drafts         │
│                       │  existing thread          │                │
├───────────────────────────────────────────────────────────────────┤
│ 3 drafts · summary [................................]  Submit ▾   │
└───────────────────────────────────────────────────────────────────┘
```

Important interactions:

- Clicking a finding scrolls to and highlights its anchor.
- Automated findings start as suggestions, not review comments.
- **Accept** creates an editable local draft; **Dismiss** records the dismissal for the current snapshot.
- Clicking a comment gutter icon creates a manual draft.
- Drafts and findings visibly distinguish manual, automated, stale, and unpostable states.
- Submit opens a final confirmation containing every inline comment, the summary, the event, and the pinned head SHA.
- Keyboard navigation covers files, hunks, findings, comment editing, and the command palette.

## 5. Architecture

Use a Rust workspace with a functional core and an imperative shell:

```text
GPUI views/actions
      │
      ▼
Application controller / state machines
      │
      ├── PR service ───── GitHub gateway (`gh api`)
      ├── Snapshot service ─ Git gateway (`git` subprocess)
      ├── Review service ── backend adapter + validators
      └── Draft store ───── SQLite
```

All Git, GitHub, database, and review work runs away from the UI thread. GPUI entities hold presentation state and subscribe to typed domain events. Domain services must not depend on GPUI so most behavior can be tested without a window.

### Suggested workspace

```text
Cargo.toml
apps/zreview/               # application entry point and GPUI composition
crates/domain/              # IDs, snapshots, diffs, anchors, findings, drafts
crates/git/                 # safe git subprocess wrapper and diff construction
crates/github/              # gh wrapper, REST payloads, pagination, submission
crates/review/              # guidance discovery, review pipeline, backends
crates/store/               # SQLite migrations and repositories
crates/ui/                  # reusable GPUI controls and diff renderer
tests/fixtures/             # golden patches, fake git/gh programs, snapshots
```

Avoid a generic plugin system in the MVP. Keep traits narrow enough to add providers later.

## 6. Snapshot and diff pipeline

### Snapshot identity

Every review session is pinned to:

```rust
PrSnapshot {
    repository: RepositoryId,
    number: u64,
    base_sha: ObjectId,
    head_sha: ObjectId,
    merge_base_sha: ObjectId,
    fetched_at: SystemTime,
}
```

Findings, viewed files, draft comments, and dismissals are keyed by this snapshot, especially `head_sha`. Refreshing to a new head never silently moves a comment.

### Loading sequence

1. Resolve the repository root with `git rev-parse --show-toplevel`.
2. Parse and normalize the PR URL or infer `owner/repo` from a matching remote.
3. Fetch PR metadata with `gh api repos/{owner}/{repo}/pulls/{number}`.
4. Fetch the base and PR head into namespaced refs such as `refs/zreview/...`; never overwrite a user's branch.
5. Compute the merge base and build the review from local Git objects.
6. Fetch existing review comments with paginated REST calls.
7. Publish metadata and file summaries first, then load file contents and syntax data lazily.

The PR file-list API is useful as a fallback/validation source but must not be the source of truth: GitHub documents a 3,000-file response limit and API patches may be truncated. Local Git objects provide full old/new content and deterministic diffs.

### Diff representation

Do not make rendered strings the domain model. Parse into typed rows:

```rust
DiffFile {
    path: RepoPath,
    old_path: Option<RepoPath>,
    status: FileStatus,
    hunks: Vec<DiffHunk>,
}

DiffLine {
    kind: Context | Addition | Deletion | NoNewlineMarker,
    old_line: Option<u32>,
    new_line: Option<u32>,
    text: Arc<str>,
}

DiffAnchor {
    path: RepoPath,
    side: Left | Right,
    line: u32,
    start_line: Option<u32>,
    head_sha: ObjectId,
}
```

Gather status/rename information using NUL-delimited Git output. Generate patches with external diff drivers and color disabled. The parser must handle additions, deletions, renames, binary files, mode changes, submodules, CRLF, missing final newlines, unusual filenames, and empty files.

For GitHub submission, map `Left` to `LEFT` and `Right` to `RIGHT`, and use `line`/`side` rather than the closing-down `position` parameter. Only anchors accepted by an `AnchorValidator` can become inline review comments. Findings outside a displayed hunk remain visible but must be moved to a valid changed/context line or included in the summary.

## 7. GitHub integration

### MVP authentication

Require `gh auth login` and execute `gh api` as a subprocess:

- Reuses the user's credential storage and SSO setup.
- Avoids reading, persisting, or logging an access token.
- Makes API calls straightforward to fake in tests.

Use `std::process::Command`/Tokio process APIs with argument arrays, never shell-formatted commands. Redact request bodies from normal logs because drafts may contain sensitive code.

### Local-first drafts

Keep all drafts in SQLite until final submission. Do not create a GitHub `PENDING` review while the user is composing; this avoids conflicts with an existing pending review and makes crash recovery under app control.

At submission:

1. Refresh PR metadata.
2. Refuse to submit if `head_sha` changed; preserve everything and offer reload/re-anchor.
3. Validate every path, line, side, and body against the snapshot.
4. Show one final confirmation.
5. POST `repos/{owner}/{repo}/pulls/{number}/reviews` with:
   - `commit_id: head_sha`
   - `body`
   - `event: COMMENT | APPROVE | REQUEST_CHANGES`
   - `comments: [{ path, line, side, start_line?, start_side?, body }]`
6. Mark drafts submitted only after a successful response.
7. On validation/rate-limit/network failure, preserve all local drafts and show the API's useful error detail.

The gateway should support pagination and typed error categories: unauthenticated, forbidden/scope, not found, rate limited, stale head, validation, network, and malformed response.

## 8. Local review engine

### Guidance discovery and configuration

Discover guidance automatically when a snapshot opens. Search conventional files such as:

- `.zreview.toml` explicit includes and exclusions;
- repository-level `AGENTS.md`, `CLAUDE.md`, `CONTRIBUTING.md`, `STYLEGUIDE.md`, and similarly named style documents;
- `.github/copilot-instructions.md` and path-scoped `.github/instructions/*.instructions.md` files;
- nested `AGENTS.md` or equivalent files that apply to a changed file's directory;
- user-level guidance from the platform config directory.

Apply path-scoped instructions only to matching changed files, deduplicate files, impose size limits, and record a content hash with the review run. Before running the review, show a **Guidance** panel listing every discovered file, its scope, and whether it will be sent to the selected backend. The user can disable a discovery result or add another file without editing configuration.

A repository file such as `.zreview.toml` overrides the defaults:

```toml
[review]
instructions = ["docs/style/**/*.md"]
exclude_instructions = ["CLAUDE.md"]
exclude_files = ["vendor/**", "dist/**", "**/*.lock"]

[[review.checks]]
name = "clippy"
program = "cargo"
args = ["clippy", "--message-format=json"]
parser = "cargo-json"

[review.ai]
backend = "command"
model = "configured-by-backend"
```

Automatic guidance discovery is read-only and does not imply permission to run commands. Repository-provided commands are untrusted and must require a one-time trust decision before execution. Display the exact executable, arguments, working directory, and guidance files in the run confirmation. Do not invoke a shell or auto-run checks merely by opening a repository.

### Review stages

1. **Discover** guidance and show exactly what will be used.
2. **Select** reviewable files from the snapshot and exclusions.
3. **Run deterministic checks** in a detached temporary worktree at `head_sha`, with cancellation and bounded output.
4. **Build AI inputs** from guidance, changed hunks, and bounded surrounding/full-file context.
5. **Review in chunks**, then run a deduplication/synthesis pass for cross-file findings.
6. **Validate** structured output against a strict schema and the diff anchor map.
7. **Rank and deduplicate** by stable fingerprint.
8. **Present** findings for human acceptance, editing, or dismissal.

Suggested finding schema:

```rust
Finding {
    id: FindingId,
    snapshot: SnapshotId,
    anchor: Option<DiffAnchor>,
    severity: Info | Warning | Error,
    confidence: f32,
    title: String,
    rationale: String,
    proposed_comment: String,
    guidance_sources: Vec<GuidanceCitation>,
    fingerprint: String,
    origin: Check(String) | Ai(String),
}
```

Each finding should explain which guidance produced it. This makes the review auditable and helps reviewers reject bad interpretations.

### Backend interface

Start with one backend, but isolate it:

```rust
trait ReviewBackend {
    async fn review(&self, request: ReviewRequest, events: ReviewEventSink)
        -> Result<Vec<RawFinding>, ReviewError>;
}
```

The initial review backend still needs to be selected. GitHub's `gh` CLI covers PR access and submission, but it is not itself the engine that evaluates code against guidance. Viable first adapters are a local coding-agent CLI with structured output, a direct hosted-model API, or GitHub Models if that is the intended hosted provider. Whichever is selected, the process receives bounded review material and has no need to mutate the user's checkout.

Treat source code and repository guidance as untrusted model input. Delimit them clearly, give the model no posting credential or GitHub tool, validate all output, and require human approval. The review engine must never call the GitHub submission service itself.

## 9. Persistence and state

Use bundled SQLite with migrations. Store:

- Repository identity and last-opened PRs.
- Snapshot metadata and file fingerprints.
- Viewed state.
- Review runs and findings.
- Accepted/dismissed finding state.
- Draft comments and review summary.
- Trusted repository/check-command decisions.
- Non-secret UI preferences.

Do not store GitHub tokens. Put logs and cached source material in separate locations with a clear retention policy and a **Clear review data** action.

Important state machines:

- `Session`: Opening → Loading → Ready → Refreshing / Failed.
- `ReviewRun`: Queued → Running(stage) → Cancelling → Complete / Failed / Cancelled.
- `Draft`: Editing → Valid → Stale / Invalid → Submitting → Submitted / Failed.

Persist draft changes eagerly with a short debounce and transactional writes.

## 10. Delivery phases

### Phase 0 — risk-reduction spikes

Build throwaway or isolated proofs for the four highest-risk areas:

1. GPUI window with a virtualized 100k-row diff, text selection, scrolling, keyboard focus, and an inserted comment editor.
2. Golden diff parser and line/side anchor mapper.
3. Private/fork PR metadata and ref fetch using `git` + authenticated `gh` without touching user branches.
4. One review backend returning strict structured findings with cancellation and progress.

Exit when each spike has a measured result and no architectural blocker. In particular, confirm the desired Zed-like diff experience can be built on GPUI without importing GPL Zed editor crates.

### Phase 1 — native shell and domain foundation

- Rust workspace, pinned toolchain/dependencies, linting, formatting, and CI.
- GPUI app lifecycle, theme tokens, command/actions layer, app menu, and error surface.
- Typed process runner, cancellation, tracing with redaction, config directories.
- SQLite schema and migrations.

**Exit:** the signed-off architecture is represented in compilable crates and the app can open an empty session reliably.

### Phase 2 — PR snapshot and diff viewer

- Open local repository + PR URL/number.
- `gh` preflight and metadata.
- Namespaced Git fetch, merge-base calculation, diff model/parser.
- File tree, unified virtualized diff, lazy syntax highlighting, navigation, refresh.
- Existing comments rendered read-only.

**Exit:** representative public, private, and fork PRs render complete diffs; large fixture scrolling stays responsive.

### Phase 3 — manual review workflow

- Inline composer and local drafts.
- Viewed files, draft queue, summary editor, keyboard flow.
- Anchor validation, stale-head detection, crash recovery.
- Batch submission for comment/approve/request-changes.

**Exit:** a user can complete a real PR review without the automated engine, and submission failures never lose text.

### Phase 4 — guided automated review

- `.zreview.toml` and guidance preview.
- Repository trust flow and optional deterministic checks.
- First backend adapter, progress/cancel/retry, schema validation.
- Finding queue, citations, accept/edit/dismiss, deduplication.

**Exit:** generated findings are reproducible enough to audit, cannot bypass human approval, and become ordinary editable drafts.

### Phase 5 — hardening and distribution

- Accessibility pass, keyboard-only pass, telemetry decision, privacy documentation.
- Performance profiling and memory bounds.
- macOS signing, notarization, application bundle, and `zreview` terminal helper.
- Upgrade/migration tests, crash diagnostics, release CI, and user documentation.

**Exit:** install, upgrade, authentication, review, recovery, and uninstall are documented and tested on a clean machine.

### Phase 6 — post-MVP

Prioritize from usage data: split diff, multiline comments, thread replies/resolution, commit-by-commit review, local changes, GitHub Enterprise, additional review backends, and image diffs.

## 11. Test strategy

### Unit and golden tests

- URL/remote parsing, path validation, API payloads, pagination, and error mapping.
- Diff parser fixtures for every file state and edge case.
- Bidirectional displayed-row ↔ GitHub-anchor mapping.
- Guidance globs, config precedence, trust decisions, schema validation, and deduplication.
- State-machine transitions, especially cancellation and stale snapshots.

### Contract tests

Use fake `git`, `gh`, and review executables that record argument arrays/stdin and emit fixtures. Assert that no shell is used, secrets are redacted, pagination completes, and exact GitHub review JSON is produced.

### Integration tests

Create temporary Git repositories with base/head histories, renames, binary files, fork-like remotes, and moving heads. Test draft recovery and SQLite migrations. Keep optional live GitHub tests behind a separate credentialed CI job against a disposable repository.

### UI tests

Use GPUI's test support for actions, focus, keyboard navigation, async state updates, and accessibility labels. Add screenshot tests for a small stable set of themes/layouts, but prefer semantic assertions for behavior.

### Performance budgets

Establish budgets during Phase 0, then enforce benchmarks for:

- first useful render from cached Git objects;
- scrolling a 100k-line diff without visible stalls;
- bounded memory when only a subset of files is visible;
- cancellation latency for Git, checks, and review backends;
- no synchronous subprocess/database work on the UI thread.

## 12. Main risks and mitigations

| Risk | Mitigation |
|---|---|
| GPUI is pre-1.0 and documentation is limited | Pin revisions, wrap it behind the `ui` crate, prove virtualization first, upgrade intentionally. |
| Reusing Zed components changes licensing or creates heavy coupling | Depend only on Apache GPUI/app-owned components unless a deliberate licensing decision says otherwise. |
| GitHub and local diffs disagree | Pin SHAs, derive content locally, validate anchors, submit `commit_id`, and block stale submissions. |
| Large PRs overwhelm UI/model context | Virtualize, lazy-load, chunk review inputs, cap concurrency/output, and expose exclusions. |
| AI invents issues or obeys prompt injection | No posting tools, strict output schema, guidance citations, anchor validation, and mandatory human acceptance. |
| Repository config executes malicious commands | Explicit trust prompt, no shell, visible argv, detached worktree, and no automatic execution. |
| Subprocess/API failure loses a review | Local transactional drafts, idempotent state transitions, and mark submitted only after success. |
| Private source leaves the machine unexpectedly | Make backend/data policy explicit before each backend is enabled; document and display what context is sent. |

## 13. Decision status

### Confirmed

1. Permissive licensing; target dual MIT/Apache-2.0 and avoid GPL dependencies.
2. macOS only.
3. GitHub integration and authentication through `gh`.
4. Sending review context to a hosted model is acceptable when clearly disclosed.
5. Guidance discovery is automatic, transparent, path-aware, and overridable.
6. Existing local clone, github.com, and new batched reviews remain the MVP defaults.

### Still open

1. Which engine performs the automated code review? `gh` handles GitHub transport, but a model/backend is still needed. Choose one of:
   - a local coding-agent CLI such as Claude Code, Codex, or Pi;
   - a direct hosted API such as Anthropic or OpenAI;
   - GitHub Models, if "GitHub" was intended to select the model provider.

## 14. Reference points

- [Codiff](https://github.com/nkzw-tech/codiff): product/workflow reference; its MIT-licensed implementation demonstrates useful patterns such as local Git content, `gh api`, local drafts, head-aware review submission, and preserving comments after API errors.
- [GPUI README](https://github.com/zed-industries/zed/tree/main/crates/gpui): framework status, platform setup, entities, elements, actions, async executor, and test support.
- [Zed `ui` crate](https://github.com/zed-industries/zed/tree/main/crates/ui): useful design reference, but with a different license from GPUI.
- [GPUI Component](https://github.com/longbridge/gpui-component): optional Apache-2.0 component set.
- [GitHub pull request reviews API](https://docs.github.com/en/rest/pulls/reviews): create and submit review behavior.
- [GitHub pull request review comments API](https://docs.github.com/en/rest/pulls/comments): current `line`/`side` anchor fields.
- [GitHub pull request files API](https://docs.github.com/en/rest/pulls/pulls#list-pull-requests-files): documented 3,000-file response limit.
