//! Tauri commands exposed to the frontend, and the specta builder that types them.

use std::sync::Mutex;

use app::{SessionPhase, lock};
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
        toggle_viewed
    ])
}

/// Loads the demo session into `model`, reporting each stage's label to `report`.
///
/// Takes the reporter as a plain callback so it can be tested without a Tauri channel.
fn run_load(model: &Mutex<app::SessionModel>, report: &dyn Fn(&str)) {
    let result = session::load(&SessionRequest::Demo, &ReviewStorage::Disabled, &|stage| {
        let _ = lock(model).set_stage(stage.label());
        report(stage.label());
    });
    lock(model).finish(result);
}

/// Loads the demo session into `model`, unless it has already finished loading.
///
/// What makes `open_session` idempotent under a repeated call, such as React
/// `StrictMode`'s double mount effect.
fn load_if_pending(model: &Mutex<app::SessionModel>, report: &dyn Fn(&str)) {
    if lock(model).is_loading() {
        run_load(model, report);
    }
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
    let SessionPhase::Ready(review) = guard.phase() else {
        unreachable!("select_file cannot move the model out of Ready")
    };
    Ok(dto::project_file(review.session(), index).expect("index bounds-checked above"))
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

/// Loads the demo review session, reporting each stage on `on_stage` as it goes.
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
    let model = std::sync::Arc::clone(&state.0);
    tauri::async_runtime::spawn_blocking(move || {
        load_if_pending(&model, &|stage| {
            let _ = on_stage.send(stage.to_owned());
        });
    })
    .await
    .expect("the load task should not panic");

    let guard = lock(&state.0);
    match guard.phase() {
        SessionPhase::Ready(review) => Ok(dto::project_snapshot(review.session())),
        SessionPhase::Failed(failure) => Err(failure.into()),
        SessionPhase::Loading { .. } => {
            unreachable!("finish() always leaves the model Ready or Failed")
        }
    }
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
    select_file_on_model(&state.0, index as usize)
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
    toggle_viewed_on_model(&state.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loaded_model() -> Mutex<app::SessionModel> {
        let model = Mutex::new(app::SessionModel::loading(
            SessionRequest::Demo.description(),
        ));
        run_load(&model, &|_stage| {});
        model
    }

    #[test]
    fn run_load_reports_starting_then_reaches_ready() {
        let model = Mutex::new(app::SessionModel::loading(
            SessionRequest::Demo.description(),
        ));
        let stages = Mutex::new(Vec::new());
        run_load(&model, &|stage| {
            stages.lock().unwrap().push(stage.to_owned());
        });

        assert_eq!(*stages.lock().unwrap(), vec!["Starting".to_owned()]);
        assert!(matches!(lock(&model).phase(), SessionPhase::Ready(_)));
    }

    #[test]
    fn load_if_pending_does_not_reload_a_ready_model() {
        let model = loaded_model();
        toggle_viewed_on_model(&model).expect("session is ready");

        load_if_pending(&model, &|_stage| {
            panic!("a ready model must not be reloaded");
        });

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

    #[test]
    fn toggle_viewed_on_model_round_trips_through_sidebar_dto() {
        let model = loaded_model();

        let sidebar = toggle_viewed_on_model(&model).expect("session is ready");
        assert_eq!(sidebar.viewed_count, 1);

        let sidebar = toggle_viewed_on_model(&model).expect("session is ready");
        assert_eq!(sidebar.viewed_count, 0);
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
}
