//! Tauri commands exposed to the frontend, and the specta builder that types them.

use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};

use app::{SessionPhase, lock};
use domain::DiffSide;
use session::{ReviewStorage, SessionRequest};
use tauri::ipc::Channel;
use tauri_specta::collect_commands;

use crate::{ManagedSession, dto};

/// The specta builder, shared between the invoke handler and the bindings export.
#[must_use]
pub fn specta_builder() -> tauri_specta::Builder<tauri::Wry> {
    tauri_specta::Builder::<tauri::Wry>::new().commands(collect_commands![
        open_session,
        select_file,
        toggle_viewed,
        describe_session,
        edit_draft,
        discard_draft,
        reanchor_draft,
    ])
}

/// Loads `request` into `model`, reporting each stage's label to `report`.
///
/// Takes the reporter as a plain callback so it can be tested without a Tauri channel.
fn run_load(
    model: &Mutex<app::SessionModel>,
    request: &SessionRequest,
    storage: &ReviewStorage,
    report: &dyn Fn(&str),
) {
    let result = session::load(request, storage, &|stage| {
        let _ = lock(model).set_stage(stage.label());
        report(stage.label());
    });
    lock(model).finish(result);
}

/// Loads `request` into `model`, letting exactly one concurrent caller do it.
///
/// `load_started` is swapped exactly once; the caller that loses the race
/// waits for the winner's load to finish rather than returning early, which is
/// what keeps `open_session`'s later `phase()` read honest under a repeated
/// call, such as React `StrictMode`'s double mount effect.
fn load_if_pending(
    model: &Mutex<app::SessionModel>,
    load_started: &AtomicBool,
    request: &SessionRequest,
    storage: &ReviewStorage,
    report: &dyn Fn(&str),
) {
    if load_started.swap(true, Ordering::AcqRel) {
        // Bounded so a loader that dies without finishing fails loudly here.
        let deadline = std::time::Instant::now() + std::time::Duration::from_mins(10);
        while lock(model).is_loading() {
            assert!(
                std::time::Instant::now() < deadline,
                "the winning loader never finished"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        return;
    }
    run_load(model, request, storage, report);
}

fn select_file_on_model(
    model: &Mutex<app::SessionModel>,
    index: usize,
) -> Result<dto::FileDetailDto, dto::SessionFailureDto> {
    let mut guard = lock(model);
    let in_range = match guard.phase() {
        SessionPhase::Ready(review) => index < review.session().files().len(),
        SessionPhase::Loading { .. } | SessionPhase::Failed(_) => {
            return Err(dto::command_failure("the session is not ready"));
        }
    };
    if !in_range {
        return Err(dto::command_failure("could not select that file"));
    }
    // A same-index call reports false; that is a no-op, not a failure.
    guard.select_file(index);
    let write_failure = guard.draft_write_failure();
    let SessionPhase::Ready(review) = guard.phase() else {
        unreachable!("select_file cannot move the model out of Ready")
    };
    let mut detail =
        dto::project_file(review.session(), index).expect("index bounds-checked above");
    detail.drafts.write_failure = write_failure;
    Ok(detail)
}

fn toggle_viewed_on_model(
    model: &Mutex<app::SessionModel>,
) -> Result<dto::SidebarDto, dto::SessionFailureDto> {
    let mut guard = lock(model);
    let SessionPhase::Ready(_) = guard.phase() else {
        return Err(dto::command_failure("the session is not ready"));
    };
    guard.toggle_viewed();
    let SessionPhase::Ready(review) = guard.phase() else {
        unreachable!("toggle_viewed cannot move the model out of Ready")
    };
    Ok(dto::project_sidebar(review.session()))
}

/// Projects the drafts on `file_index` from an already-locked, `Ready` model.
///
/// # Panics
///
/// Panics if the model is not `Ready`. Every caller here has just checked that
/// under the same lock.
fn drafts_snapshot(model: &app::SessionModel, file_index: usize) -> dto::DraftsDto {
    let write_failure = model.draft_write_failure();
    let SessionPhase::Ready(review) = model.phase() else {
        unreachable!("checked Ready under the same lock")
    };
    let mut drafts = dto::project_drafts(review.session(), file_index);
    drafts.write_failure = write_failure;
    drafts
}

/// Edits the draft over `start.min(end)..=start.max(end)` of `file_index`,
/// entirely under one lock so the check against the selected file and the
/// mutation it guards cannot straddle a file switch.
///
/// A `file_index` that no longer names the selected file is a late command
/// for a file the reviewer has since left. It is dropped rather than applied
/// to whatever is now selected. `accepted` comes back `false` and `drafts`
/// reflects the file that actually is selected.
fn edit_draft_on_model(
    model: &Mutex<app::SessionModel>,
    file_index: usize,
    start: usize,
    end: usize,
    body: String,
) -> Result<dto::DraftEditOutcomeDto, dto::SessionFailureDto> {
    let mut guard = lock(model);
    let selected = match guard.phase() {
        SessionPhase::Ready(review) => review.session().selected_file_index(),
        SessionPhase::Loading { .. } | SessionPhase::Failed(_) => {
            return Err(dto::command_failure("the session is not ready"));
        }
    };
    if file_index != selected {
        return Ok(dto::DraftEditOutcomeDto {
            accepted: false,
            drafts: drafts_snapshot(&guard, selected),
        });
    }
    let accepted = guard.draft_edited(start.min(end)..=start.max(end), body);
    Ok(dto::DraftEditOutcomeDto {
        accepted,
        drafts: drafts_snapshot(&guard, selected),
    })
}

/// Discards the draft on `row` of `file_index`, dropped silently when
/// `file_index` is no longer selected. See [`edit_draft_on_model`].
fn discard_draft_on_model(
    model: &Mutex<app::SessionModel>,
    file_index: usize,
    row: usize,
) -> Result<dto::DraftsDto, dto::SessionFailureDto> {
    let mut guard = lock(model);
    let selected = match guard.phase() {
        SessionPhase::Ready(review) => review.session().selected_file_index(),
        SessionPhase::Loading { .. } | SessionPhase::Failed(_) => {
            return Err(dto::command_failure("the session is not ready"));
        }
    };
    if file_index == selected {
        // A false return means the row held no draft; that is a no-op, not a failure.
        guard.draft_discarded(row);
    }
    Ok(drafts_snapshot(&guard, selected))
}

/// Moves the stale draft identified by `(path, side, line)` onto `row` of
/// `file_index`, dropped silently when `file_index` is no longer selected.
/// See [`edit_draft_on_model`].
///
/// The triple takes the place of the private key `Drafts` indexes stale drafts
/// by; this looks the matching anchor up from the selected file's own stale
/// drafts rather than trusting one reconstructed from IPC input.
fn reanchor_draft_on_model(
    model: &Mutex<app::SessionModel>,
    file_index: usize,
    path: &str,
    side: DiffSide,
    line: u32,
    row: usize,
) -> Result<dto::DraftsDto, dto::SessionFailureDto> {
    let mut guard = lock(model);
    let selected = match guard.phase() {
        SessionPhase::Ready(review) => review.session().selected_file_index(),
        SessionPhase::Loading { .. } | SessionPhase::Failed(_) => {
            return Err(dto::command_failure("the session is not ready"));
        }
    };
    if file_index != selected {
        return Ok(drafts_snapshot(&guard, selected));
    }
    let SessionPhase::Ready(review) = guard.phase() else {
        unreachable!("checked Ready above")
    };
    let stale = review
        .session()
        .drafts()
        .for_path(path)
        .find(|draft| draft.is_stale && draft.anchor.side == side && draft.anchor.line == line)
        .map(|draft| draft.anchor.clone());
    let Some(stale) = stale else {
        return Err(dto::command_failure("that draft is no longer stale"));
    };
    if !guard.draft_reanchored(&stale, row) {
        return Err(dto::command_failure("that row cannot hold a comment"));
    }
    Ok(drafts_snapshot(&guard, selected))
}

/// Loads the review session the window was opened with, reporting each stage on
/// `on_stage` as it goes.
///
/// # Errors
///
/// Returns the failure the loader reported, projected for the frontend.
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub async fn open_session(
    state: tauri::State<'_, ManagedSession>,
    on_stage: Channel<String>,
) -> Result<dto::SessionSnapshotDto, dto::SessionFailureDto> {
    let model = std::sync::Arc::clone(&state.model);
    let load_started = std::sync::Arc::clone(&state.load_started);
    let request = state.request.clone();
    let storage = state.storage.clone();
    tauri::async_runtime::spawn_blocking(move || {
        load_if_pending(&model, &load_started, &request, &storage, &|stage| {
            let _ = on_stage.send(stage.to_owned());
        });
    })
    .await
    .expect("the load task should not panic");

    let guard = lock(&state.model);
    match guard.phase() {
        SessionPhase::Ready(review) => Ok(dto::project_snapshot(review.session())),
        SessionPhase::Failed(failure) => Err(failure.into()),
        SessionPhase::Loading { .. } => {
            unreachable!("finish() always leaves the model Ready or Failed")
        }
    }
}

/// The request's own description, shown by the loading screen before anything
/// about the target session is known.
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub fn describe_session(state: tauri::State<'_, ManagedSession>) -> String {
    describe_session_on_request(&state.request)
}

fn describe_session_on_request(request: &SessionRequest) -> String {
    request.description()
}

/// Switches the displayed file and returns its rows.
///
/// # Errors
///
/// Returns a failure when the index is out of range or the session is not ready.
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub async fn select_file(
    state: tauri::State<'_, ManagedSession>,
    index: u32,
) -> Result<dto::FileDetailDto, dto::SessionFailureDto> {
    select_file_on_model(&state.model, index as usize)
}

/// Marks the selected file viewed, or unmarks it, and returns the fresh sidebar.
///
/// # Errors
///
/// Returns a failure when the session is not ready.
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub fn toggle_viewed(
    state: tauri::State<'_, ManagedSession>,
) -> Result<dto::SidebarDto, dto::SessionFailureDto> {
    toggle_viewed_on_model(&state.model)
}

/// Edits the draft over rows `start..=end` of `file_index`. Persists the new
/// text on every call. That is what makes a keystroke survive a crash.
///
/// # Errors
///
/// Returns a failure when the session is not ready. An unanchorable span is not
/// an error. It comes back as `accepted: false` on the outcome, and neither is a
/// `file_index` that has since been navigated away from; the outcome reflects
/// the file that is actually selected.
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub fn edit_draft(
    state: tauri::State<'_, ManagedSession>,
    file_index: u32,
    start: u32,
    end: u32,
    body: String,
) -> Result<dto::DraftEditOutcomeDto, dto::SessionFailureDto> {
    edit_draft_on_model(
        &state.model,
        file_index as usize,
        start as usize,
        end as usize,
        body,
    )
}

/// Discards the draft on `row` of `file_index`.
///
/// # Errors
///
/// Returns a failure when the session is not ready.
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub fn discard_draft(
    state: tauri::State<'_, ManagedSession>,
    file_index: u32,
    row: u32,
) -> Result<dto::DraftsDto, dto::SessionFailureDto> {
    discard_draft_on_model(&state.model, file_index as usize, row as usize)
}

/// Moves a stale draft, named by the position it was written against, onto
/// `row` of `file_index`.
///
/// # Errors
///
/// Returns a failure when the session is not ready, no stale draft matches
/// `(path, side, line)`, or `row` cannot carry a comment.
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub fn reanchor_draft(
    state: tauri::State<'_, ManagedSession>,
    file_index: u32,
    path: String,
    side: dto::DiffSideDto,
    line: u32,
    row: u32,
) -> Result<dto::DraftsDto, dto::SessionFailureDto> {
    reanchor_draft_on_model(
        &state.model,
        file_index as usize,
        &path,
        side.into(),
        line,
        row as usize,
    )
}

#[cfg(test)]
mod tests {
    use std::{ffi::OsStr, path::Path, process::Command, sync::Arc, thread};

    use tempfile::TempDir;

    use super::*;

    /// A sink whose writes always fail, for exercising `draft_write_failure`.
    struct FailingSink;

    impl domain::ReviewStateSink for FailingSink {
        fn save(&self, _anchor: &domain::DiffAnchor, _body: &str) {}
        fn discard(&self, _anchor: &domain::DiffAnchor) {}
        fn save_summary(&self, _head_sha: &str, _body: &str) {}
        fn save_provenance(
            &self,
            _anchor: &domain::DiffAnchor,
            _provenance: &domain::FindingProvenance,
        ) {
        }
        fn dismiss_finding(&self, _head_sha: &str, _fingerprint: &str) {}
        fn clear_submitted(&self, _head_sha: &str, _anchors: &[domain::DiffAnchor]) {}
        fn failure(&self) -> Option<String> {
            Some("disk is full".to_owned())
        }
    }

    fn loaded_model() -> Mutex<app::SessionModel> {
        let model = Mutex::new(app::SessionModel::loading(
            SessionRequest::Demo.description(),
        ));
        run_load(
            &model,
            &SessionRequest::Demo,
            &ReviewStorage::Disabled,
            &|_stage| {},
        );
        model
    }

    /// A repository with a `main` branch and a `feature` branch that adds
    /// `feature.txt` as four addition lines in one hunk, which is enough to
    /// exercise both single-row and span drafts.
    fn temporary_repository() -> TempDir {
        let repository = TempDir::new().unwrap();
        let path = repository.path();
        git(path, ["init", "--quiet", "--initial-branch=main"]);
        git(path, ["config", "user.name", "ZReview Test"]);
        git(path, ["config", "user.email", "zreview@example.invalid"]);
        std::fs::write(path.join("shared.txt"), "fork point\n").unwrap();
        git(path, ["add", "."]);
        git(path, ["commit", "--quiet", "-m", "fork point"]);
        git(path, ["checkout", "--quiet", "-b", "feature"]);
        std::fs::write(
            path.join("feature.txt"),
            "line one\nline two\nline three\nline four\n",
        )
        .unwrap();
        git(path, ["add", "."]);
        git(path, ["commit", "--quiet", "-m", "feature"]);
        git(path, ["checkout", "--quiet", "main"]);
        repository
    }

    fn git<I, S>(repository: &Path, args: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let status = Command::new("git")
            .arg("-C")
            .arg(repository)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success());
    }

    fn local_request(repository: &TempDir) -> SessionRequest {
        SessionRequest::LocalComparison {
            repository: repository.path().to_path_buf(),
            base: "main".to_owned(),
            head: "feature".to_owned(),
        }
    }

    fn local_model(request: &SessionRequest, storage: &ReviewStorage) -> Mutex<app::SessionModel> {
        let model = Mutex::new(app::SessionModel::loading(request.description()));
        run_load(&model, request, storage, &|_stage| {});
        model
    }

    #[test]
    fn run_load_reports_starting_then_reaches_ready() {
        let model = Mutex::new(app::SessionModel::loading(
            SessionRequest::Demo.description(),
        ));
        let stages = Mutex::new(Vec::new());
        run_load(
            &model,
            &SessionRequest::Demo,
            &ReviewStorage::Disabled,
            &|stage| {
                stages.lock().unwrap().push(stage.to_owned());
            },
        );

        assert_eq!(*stages.lock().unwrap(), vec!["Starting".to_owned()]);
        assert!(matches!(lock(&model).phase(), SessionPhase::Ready(_)));
    }

    #[test]
    fn load_if_pending_does_not_reload_a_ready_model() {
        let model = loaded_model();
        toggle_viewed_on_model(&model).expect("session is ready");
        // Standing in for a `load_started` a prior real `open_session` call
        // already flipped. This model was made Ready by `run_load` directly,
        // never through `load_if_pending`.
        let load_started = AtomicBool::new(true);

        load_if_pending(
            &model,
            &load_started,
            &SessionRequest::Demo,
            &ReviewStorage::Disabled,
            &|_stage| {
                panic!("a ready model must not be reloaded");
            },
        );

        let guard = lock(&model);
        let SessionPhase::Ready(review) = guard.phase() else {
            panic!("model should still be ready");
        };
        assert_eq!(dto::project_sidebar(review.session()).viewed_count, 1);
    }

    /// `load_started` exists so two callers racing to open the same window
    /// only ever run the loader once.
    #[test]
    fn load_if_pending_lets_exactly_one_concurrent_caller_load() {
        let model = Arc::new(Mutex::new(app::SessionModel::loading(
            SessionRequest::Demo.description(),
        )));
        let load_started = Arc::new(AtomicBool::new(false));
        let stage_reports = Arc::new(Mutex::new(0_u32));

        let handles = (0..2)
            .map(|_| {
                let model = Arc::clone(&model);
                let load_started = Arc::clone(&load_started);
                let stage_reports = Arc::clone(&stage_reports);
                thread::spawn(move || {
                    load_if_pending(
                        &model,
                        &load_started,
                        &SessionRequest::Demo,
                        &ReviewStorage::Disabled,
                        &|_stage| *stage_reports.lock().unwrap() += 1,
                    );
                })
            })
            .collect::<Vec<_>>();
        for handle in handles {
            handle.join().unwrap();
        }

        // The demo reports exactly one stage; a second load would double this.
        assert_eq!(*stage_reports.lock().unwrap(), 1);
        assert!(matches!(lock(&model).phase(), SessionPhase::Ready(_)));

        // Well-formed afterward. Nothing still racing clobbers a mutation made now.
        toggle_viewed_on_model(&model).expect("session is ready");
        let guard = lock(&model);
        let SessionPhase::Ready(review) = guard.phase() else {
            panic!("model should still be ready");
        };
        assert_eq!(dto::project_sidebar(review.session()).viewed_count, 1);
    }

    #[test]
    fn select_file_on_model_loads_the_stress_file_at_index_zero() {
        let model = loaded_model();
        let detail = select_file_on_model(&model, 0).expect("index zero is in range");
        assert_eq!(detail.rows.len(), 100_000);
    }

    #[test]
    fn select_file_on_model_reselecting_the_current_file_is_a_no_op_success() {
        let model = loaded_model();
        select_file_on_model(&model, 0).expect("first selection succeeds");
        let detail =
            select_file_on_model(&model, 0).expect("reselecting the same file also succeeds");
        assert_eq!(detail.index, 0);
    }

    #[test]
    fn select_file_on_model_reports_out_of_range() {
        let model = loaded_model();
        let error = select_file_on_model(&model, 999).unwrap_err();
        assert_eq!(error.summary, "could not select that file");
    }

    /// `select_file` is how the frontend learns about drafts on the file it
    /// just switched to, so the sink's failure has to ride along with it too.
    #[test]
    fn select_file_on_model_carries_the_sinks_write_failure() {
        let repository = temporary_repository();
        let loaded = session::load(
            &local_request(&repository),
            &ReviewStorage::Disabled,
            &|_| {},
        )
        .expect("the repository loads");
        let model = Mutex::new(app::SessionModel::loading("test"));
        lock(&model).finish(Ok(domain::LoadedSession {
            session: loaded.session,
            review_sink: Some(Box::new(FailingSink)),
            submitter: None,
        }));

        let detail = select_file_on_model(&model, 0).expect("index zero is in range");

        assert_eq!(detail.drafts.write_failure.as_deref(), Some("disk is full"));
    }

    #[test]
    fn toggle_viewed_on_model_round_trips_through_sidebar_dto() {
        let model = loaded_model();

        let sidebar = toggle_viewed_on_model(&model).expect("session is ready");
        assert_eq!(sidebar.viewed_count, 1);

        let sidebar = toggle_viewed_on_model(&model).expect("session is ready");
        assert_eq!(sidebar.viewed_count, 0);
    }

    #[test]
    fn describe_session_on_request_defers_to_the_requests_own_description() {
        let request = SessionRequest::LocalComparison {
            repository: Path::new("/tmp/repository").to_path_buf(),
            base: "main".to_owned(),
            head: "feature".to_owned(),
        };

        assert_eq!(describe_session_on_request(&request), request.description());
    }

    #[test]
    fn bindings_are_current() {
        let committed = include_str!("../../src/bindings.ts");
        let temp_path =
            std::env::temp_dir().join(format!("desktop-bindings-{}.ts", std::process::id()));

        specta_builder()
            .export(specta_typescript::Typescript::default(), &temp_path)
            .expect("failed to export typescript bindings");
        let generated =
            std::fs::read_to_string(&temp_path).expect("failed to read generated bindings");
        std::fs::remove_file(&temp_path).ok();

        assert_eq!(
            generated, committed,
            "bindings.ts is stale; regenerate it from specta_builder()"
        );
    }

    #[test]
    fn edit_draft_on_model_creates_a_draft_at_the_row() {
        let repository = temporary_repository();
        let model = local_model(&local_request(&repository), &ReviewStorage::Disabled);

        let outcome =
            edit_draft_on_model(&model, 0, 0, 0, "needs a test".to_owned()).expect("session ready");

        assert!(outcome.accepted);
        assert_eq!(outcome.drafts.anchored.len(), 1);
        assert_eq!(outcome.drafts.anchored[0].row, 0);
        assert_eq!(outcome.drafts.anchored[0].body, "needs a test");
        assert!(!outcome.drafts.anchored[0].is_proposed);
    }

    /// A range is keyed at its last row, the same place GitHub anchors it.
    #[test]
    fn edit_draft_on_model_anchors_a_span_at_its_end_row() {
        let repository = temporary_repository();
        let model = local_model(&local_request(&repository), &ReviewStorage::Disabled);

        let outcome =
            edit_draft_on_model(&model, 0, 0, 2, "this block".to_owned()).expect("session ready");

        assert!(outcome.accepted);
        assert_eq!(outcome.drafts.anchored.len(), 1);
        assert_eq!(outcome.drafts.anchored[0].row, 2);
    }

    /// A span given backwards is normalized before it reaches the domain, so
    /// it behaves exactly like the same span given forwards.
    #[test]
    fn edit_draft_on_model_normalizes_an_inverted_span() {
        let repository = temporary_repository();
        let model = local_model(&local_request(&repository), &ReviewStorage::Disabled);

        let outcome =
            edit_draft_on_model(&model, 0, 3, 1, "inverted".to_owned()).expect("session ready");
        assert!(outcome.accepted);
        assert_eq!(outcome.drafts.anchored.len(), 1);
        assert_eq!(outcome.drafts.anchored[0].row, 3);

        let drafts = discard_draft_on_model(&model, 0, 3).expect("session ready");
        assert!(drafts.anchored.is_empty());
    }

    #[test]
    fn edit_draft_on_model_removes_the_draft_when_emptied() {
        let repository = temporary_repository();
        let model = local_model(&local_request(&repository), &ReviewStorage::Disabled);
        edit_draft_on_model(&model, 0, 0, 0, "oops".to_owned()).unwrap();

        let outcome = edit_draft_on_model(&model, 0, 0, 0, String::new()).expect("session ready");

        assert!(outcome.accepted);
        assert!(outcome.drafts.anchored.is_empty());
    }

    #[test]
    fn edit_draft_on_model_reports_unaccepted_for_a_row_outside_the_diff() {
        let repository = temporary_repository();
        let model = local_model(&local_request(&repository), &ReviewStorage::Disabled);

        let outcome =
            edit_draft_on_model(&model, 0, 999, 999, "nowhere".to_owned()).expect("session ready");

        assert!(!outcome.accepted);
        assert!(outcome.drafts.anchored.is_empty());
    }

    #[test]
    fn edit_draft_on_model_is_unaccepted_on_the_demo_session() {
        let model = loaded_model();

        let outcome = edit_draft_on_model(&model, 0, 0, 0, "nowhere to anchor".to_owned())
            .expect("session ready");

        assert!(
            !outcome.accepted,
            "the demo session has no head to anchor against"
        );
    }

    /// A late edit for a file the reviewer has since left is dropped, not
    /// applied to whatever is now selected.
    #[test]
    fn edit_draft_on_model_drops_an_edit_for_a_file_that_is_no_longer_selected() {
        let repository = temporary_repository();
        let model = local_model(&local_request(&repository), &ReviewStorage::Disabled);

        let outcome = edit_draft_on_model(&model, 1, 0, 0, "late edit".to_owned())
            .expect("dropped, not erred");

        assert!(!outcome.accepted);
        assert_eq!(outcome.drafts.file_index, 0);
        assert!(outcome.drafts.anchored.is_empty());
    }

    #[test]
    fn discard_draft_on_model_removes_a_draft() {
        let repository = temporary_repository();
        let model = local_model(&local_request(&repository), &ReviewStorage::Disabled);
        edit_draft_on_model(&model, 0, 0, 0, "throwaway".to_owned()).unwrap();

        let drafts = discard_draft_on_model(&model, 0, 0).expect("session ready");

        assert!(drafts.anchored.is_empty());
    }

    /// A row with no draft is a no-op, not a failure.
    #[test]
    fn discard_draft_on_model_on_an_empty_row_still_succeeds() {
        let repository = temporary_repository();
        let model = local_model(&local_request(&repository), &ReviewStorage::Disabled);

        let drafts = discard_draft_on_model(&model, 0, 0).expect("session ready");

        assert!(drafts.anchored.is_empty());
    }

    /// A late discard for a file the reviewer has since left must not touch
    /// whatever is now selected.
    #[test]
    fn discard_draft_on_model_drops_a_discard_for_a_file_that_is_no_longer_selected() {
        let repository = temporary_repository();
        let model = local_model(&local_request(&repository), &ReviewStorage::Disabled);
        edit_draft_on_model(&model, 0, 0, 0, "keep me".to_owned()).unwrap();

        let drafts = discard_draft_on_model(&model, 1, 0).expect("dropped, not erred");

        assert_eq!(drafts.file_index, 0);
        assert_eq!(
            drafts.anchored.len(),
            1,
            "the late discard must not touch it"
        );
    }

    /// The whole point of persistence. What was typed comes back after the
    /// session that typed it is gone.
    #[test]
    fn edit_draft_on_model_persists_and_restores_across_a_reload() {
        let repository = temporary_repository();
        let data = TempDir::new().unwrap();
        let storage = ReviewStorage::At(data.path().join("review-data.sqlite3"));

        {
            let model = local_model(&local_request(&repository), &storage);
            edit_draft_on_model(&model, 0, 0, 0, "worth keeping".to_owned()).unwrap();
            // Dropping joins the writer thread, so the write has landed.
        }

        let model = local_model(&local_request(&repository), &storage);
        let guard = lock(&model);
        let SessionPhase::Ready(review) = guard.phase() else {
            panic!("session should be ready");
        };
        let draft = review.session().draft_at(0, 0).expect("it should restore");
        assert_eq!(draft.body, "worth keeping");
        assert!(!draft.is_stale);
        assert!(review.session().warnings().is_empty());
    }

    /// Advancing the branch's head, without touching the file the draft is on,
    /// still invalidates it. A draft's anchor is pinned to the exact head it was
    /// written against, not merely to a line number.
    #[test]
    fn reanchor_draft_on_model_moves_a_restored_stale_draft() {
        let repository = temporary_repository();
        let data = TempDir::new().unwrap();
        let storage = ReviewStorage::At(data.path().join("review-data.sqlite3"));

        {
            let model = local_model(&local_request(&repository), &storage);
            edit_draft_on_model(&model, 0, 0, 0, "stale note".to_owned()).unwrap();
        }

        let path = repository.path();
        git(path, ["checkout", "--quiet", "feature"]);
        std::fs::write(path.join("other.txt"), "unrelated\n").unwrap();
        git(path, ["add", "."]);
        git(path, ["commit", "--quiet", "-m", "advance"]);
        git(path, ["checkout", "--quiet", "main"]);

        let model = local_model(&local_request(&repository), &storage);
        {
            let guard = lock(&model);
            let SessionPhase::Ready(review) = guard.phase() else {
                panic!("session should be ready");
            };
            assert_eq!(review.session().drafts().stale_count(), 1);
            assert!(
                review
                    .session()
                    .warnings()
                    .iter()
                    .any(|warning| warning.summary.contains("no longer match this diff")),
                "unexpected warnings: {:?}",
                review.session().warnings(),
            );
        }

        // Moved onto a different row than it was written against, on purpose.
        let drafts = reanchor_draft_on_model(&model, 0, "feature.txt", DiffSide::Right, 1, 1)
            .expect("the stale draft should move onto row 1");

        assert!(drafts.stale.is_empty());
        assert_eq!(drafts.anchored.len(), 1);
        assert_eq!(drafts.anchored[0].row, 1);
        assert_eq!(drafts.anchored[0].body, "stale note");
    }

    #[test]
    fn reanchor_draft_on_model_reports_a_key_that_matches_no_stale_draft() {
        let repository = temporary_repository();
        let model = local_model(&local_request(&repository), &ReviewStorage::Disabled);

        let error =
            reanchor_draft_on_model(&model, 0, "feature.txt", DiffSide::Right, 999, 0).unwrap_err();

        assert_eq!(error.summary, "that draft is no longer stale");
    }

    /// The key matches a real stale draft, but the row it is asked to move to
    /// cannot itself carry a comment.
    #[test]
    fn reanchor_draft_on_model_reports_when_the_target_row_cannot_hold_a_comment() {
        let repository = temporary_repository();
        let data = TempDir::new().unwrap();
        let storage = ReviewStorage::At(data.path().join("review-data.sqlite3"));

        {
            let model = local_model(&local_request(&repository), &storage);
            edit_draft_on_model(&model, 0, 0, 0, "stale note".to_owned()).unwrap();
        }

        let path = repository.path();
        git(path, ["checkout", "--quiet", "feature"]);
        std::fs::write(path.join("other.txt"), "unrelated\n").unwrap();
        git(path, ["add", "."]);
        git(path, ["commit", "--quiet", "-m", "advance"]);
        git(path, ["checkout", "--quiet", "main"]);

        let model = local_model(&local_request(&repository), &storage);

        let error =
            reanchor_draft_on_model(&model, 0, "feature.txt", DiffSide::Right, 1, 999).unwrap_err();

        assert_eq!(error.summary, "that row cannot hold a comment");
    }

    /// A late reanchor for a file the reviewer has since left must not move
    /// anything.
    #[test]
    fn reanchor_draft_on_model_drops_a_reanchor_for_a_file_that_is_no_longer_selected() {
        let repository = temporary_repository();
        let data = TempDir::new().unwrap();
        let storage = ReviewStorage::At(data.path().join("review-data.sqlite3"));

        {
            let model = local_model(&local_request(&repository), &storage);
            edit_draft_on_model(&model, 0, 0, 0, "stale note".to_owned()).unwrap();
        }

        let path = repository.path();
        git(path, ["checkout", "--quiet", "feature"]);
        std::fs::write(path.join("other.txt"), "unrelated\n").unwrap();
        git(path, ["add", "."]);
        git(path, ["commit", "--quiet", "-m", "advance"]);
        git(path, ["checkout", "--quiet", "main"]);

        let model = local_model(&local_request(&repository), &storage);

        let drafts = reanchor_draft_on_model(&model, 1, "feature.txt", DiffSide::Right, 1, 1)
            .expect("dropped, not erred");

        assert_eq!(
            drafts.stale.len(),
            1,
            "the late reanchor must not have moved it"
        );
    }
}
