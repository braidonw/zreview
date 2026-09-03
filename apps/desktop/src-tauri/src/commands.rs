//! Tauri commands exposed to the frontend, and the specta builder that types them.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use app::{
    Opened, PullRequestId, RepositoryOutcome, SessionPhase, SettingsWrite, Showing, lock, try_lock,
};
use domain::DiffSide;
use session::{ReviewStorage, SessionRequest};
use tauri::ipc::Channel;
use tauri_specta::collect_commands;

use crate::{AppRoot, ManagedHome, Window, drafts, dto, pull_requests, repositories};

/// The specta builder, shared between the invoke handler and the bindings export.
#[must_use]
pub fn specta_builder() -> tauri_specta::Builder<tauri::Wry> {
    tauri_specta::Builder::<tauri::Wry>::new().commands(collect_commands![
        describe_window,
        refresh_home,
        move_home_cursor,
        add_repositories,
        remove_repository,
        toggle_repositories_footer,
        open_row,
        return_to_home,
        return_to_session,
        open_session,
        select_file,
        toggle_viewed,
        edit_draft,
        discard_draft,
        reanchor_draft,
    ])
}

/// Re-reads the settings file, then fetches what every clone it lists has open.
///
/// A trigger that arrives while any settings action is running starts nothing.
/// It is not queued, because a refresh only ever asks what is true now, and the
/// one already running is about to answer that.
///
/// Once the fetch has landed, `database_path` is read for the Drafts count on
/// every row. A missing database means no Drafts anywhere; anything else that
/// stops it opening or reading is the failure Home shows above the list.
///
/// Takes the reporter as a plain callback so it can be tested without a Tauri
/// channel, as `run_load` does.
fn refresh_home_on_model(
    root: &AppRoot,
    settings_path: &Path,
    database_path: &Path,
    report: &dyn Fn(dto::HomeSnapshotDto),
) {
    let home = &root.home;
    let Some(_ordered) = try_lock(&home.settings_action) else {
        return;
    };
    lock(&home.model).begin_refresh();
    // Every way out from here says how the refresh went, except a panic, which
    // would otherwise leave the stamp counting repositories off for ever.
    let _abandoned = AbandonedRefresh { home };
    report_home(root, report);

    read_into_model(home, settings_path);
    // Only the settings file can have failed by here, whatever stopped the last
    // refresh having gone with the trigger that started this one.
    if lock(&home.model).failure().is_some() {
        report_home(root, report);
        return;
    }
    let (slugs, preflight_in) = to_fetch(&lock(&home.model));
    if let Some(clone) = preflight_in
        && let Err(failure) = pull_requests::preflight(&home.client, &clone)
    {
        lock(&home.model).preflight_failed(failure);
        report_home(root, report);
        return;
    }

    lock(&home.model).fetching(slugs.len());
    report_home(root, report);
    pull_requests::fetch(&home.client, &slugs, &|batch| {
        lock(&home.model).batch_fetched(batch);
        report_home(root, report);
    });
    // Evaluated before the lock is taken, so the store open and query never
    // hold the model mutex against every other command waiting on it.
    let drafts_result = drafts::read(database_path);
    lock(&home.model).drafts_read(drafts_result);
    lock(&home.model).finish_refresh(now_ms());
    report_home(root, report);
}

/// Ends a refresh that stopped without finishing.
///
/// A panic anywhere in a refresh would otherwise leave the stamp saying it is
/// still counting repositories off, with nothing left running to finish it.
struct AbandonedRefresh<'a> {
    home: &'a ManagedHome,
}

impl Drop for AbandonedRefresh<'_> {
    fn drop(&mut self) {
        // A refresh that reached its own end has already said how it went.
        lock(&self.home.model).refresh_abandoned();
    }
}

/// Hands the reporter everything Home now shows, so a list fills in as it loads.
fn report_home(root: &AppRoot, report: &dyn Fn(dto::HomeSnapshotDto)) {
    report(shown_home(root));
}

/// Everything Home now shows, with the row of the Session behind it marked.
fn shown_home(root: &AppRoot) -> dto::HomeSnapshotDto {
    // Read out and released before Home's own lock is taken, so the two are
    // never held at once and so can never be taken in two orders.
    let alive = alive_pull_request(root);
    dto::project_home(&lock(&root.home.model), alive.as_ref())
}

/// The pull request the Session behind Home is open on, when one is.
fn alive_pull_request(root: &AppRoot) -> Option<PullRequestId> {
    lock(&root.window)
        .slot
        .session()
        .and_then(app::OpenSession::pull_request)
        .cloned()
}

/// The repositories a refresh will ask about, and the clone to preflight in.
///
/// Duplicates are collapsed by the fetch itself, which is the one place that
/// knows how GitHub compares two names, so the count here is the count it will
/// answer for.
fn to_fetch(home: &app::HomeModel) -> (Vec<github::RepositorySlug>, Option<PathBuf>) {
    let mut slugs = Vec::new();
    let mut preflight_in = None;
    for entry in home.repositories() {
        let Some(slug) = entry.slug() else {
            continue;
        };
        preflight_in.get_or_insert_with(|| entry.path.clone());
        slugs.push(
            github::parse_full_name(slug).expect("a slug the github crate formatted parses back"),
        );
    }
    (github::distinct_repositories(&slugs), preflight_in)
}

/// Now, as the epoch milliseconds the stamp is worked out from.
///
/// # Panics
///
/// Panics if the system clock is set before 1970, against which no relative
/// time this screen shows would mean anything.
fn now_ms() -> i64 {
    let since_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the system clock is set after 1970");
    i64::try_from(since_epoch.as_millis()).expect("the system clock is set before the year 292M")
}

fn move_home_cursor_on_model(root: &AppRoot, move_to: dto::CursorMoveDto) -> dto::HomeSnapshotDto {
    match move_to {
        dto::CursorMoveDto::Down => lock(&root.home.model).move_cursor_down(),
        dto::CursorMoveDto::Up => lock(&root.home.model).move_cursor_up(),
    }
    shown_home(root)
}

/// Adds the folders a reviewer picked, then leaves Home showing the file.
fn add_repositories_on_model(home: &ManagedHome, settings_path: &Path, folders: &[PathBuf]) {
    let _ordered = lock(&home.settings_action);
    let picked = repositories::resolve_picked(folders);
    let write = lock(&home.model).add_repositories(settings_path, picked);
    write_then_read(home, settings_path, write);
}

/// Drops the entry listed at `path`, then leaves Home showing the file.
fn remove_repository_on_model(home: &ManagedHome, settings_path: &Path, path: &Path) {
    let _ordered = lock(&home.settings_action);
    let write = lock(&home.model).remove_repository(settings_path, path);
    write_then_read(home, settings_path, write);
}

/// Reads the settings file into the model. The caller holds the action guard.
fn read_into_model(home: &ManagedHome, settings_path: &Path) {
    let read = repositories::read(settings_path);
    lock(&home.model).refreshed(read);
}

/// Writes what an action asked for and reads back the file it wrote.
///
/// The read runs whether or not the write worked, so what Home shows is always
/// what the file actually holds. A write that failed is its own line above the
/// list rather than a screen in front of it.
fn write_then_read(home: &ManagedHome, settings_path: &Path, write: Option<SettingsWrite>) {
    if let Some(write) = write {
        let written = repositories::write(settings_path, write.repositories);
        lock(&home.model).write_finished(written);
    }
    read_into_model(home, settings_path);
}

/// Runs `action` against the settings file, then projects what Home now shows.
///
/// A machine with no home directory has nowhere to keep the file, which is a
/// whole-Home failure rather than something an action can work around.
fn on_settings_file(root: &AppRoot, action: impl FnOnce(&AppRoot, &Path)) -> dto::HomeSnapshotDto {
    match repositories::settings_path() {
        Ok(path) => action(root, &path),
        Err(failure) => lock(&root.home.model).refreshed(Err(failure)),
    }
    shown_home(root)
}

/// Runs Home's file and Git work away from the UI thread.
///
/// The only failure is the task not finishing at all, which the frontend shows
/// rather than waiting on a promise that will never settle.
async fn off_the_ui_thread(
    work: impl FnOnce() -> dto::HomeSnapshotDto + Send + 'static,
) -> Result<dto::HomeSnapshotDto, dto::SessionFailureDto> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|error| {
            dto::command_failure(format!("Home could not finish that action: {error}"))
        })
}

/// Held for the whole of one Session load, so no two ever run at once.
///
/// Opening a different row drops the Session that was loading, but not the load
/// itself, which runs on until it finishes. A load runs Git and `gh` against a
/// clone, and the load that replaces it usually reaches the same clone, so the
/// new one waits here rather than fetching over the old one.
static SESSION_LOAD: Mutex<()> = Mutex::new(());

/// Loads `request` into `model`, reporting each stage's label to `report`.
///
/// Takes the reporter as a plain callback so it can be tested without a Tauri channel.
fn run_load(
    model: &Mutex<app::SessionModel>,
    request: &SessionRequest,
    storage: &ReviewStorage,
    report: &dyn Fn(&str),
) {
    let _ordered = lock(&SESSION_LOAD);
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
    state: tauri::State<'_, AppRoot>,
    on_stage: Channel<String>,
) -> Result<dto::SessionSnapshotDto, dto::SessionFailureDto> {
    // Taken out whole, so nothing holds the window's lock across the load, and
    // a Session replaced part way through still reports its own result.
    let (model, load_started, request, storage) = {
        let window = lock(&state.window);
        let session = window
            .session
            .as_ref()
            .ok_or_else(|| dto::command_failure("no session is open"))?;
        (
            Arc::clone(&session.model),
            Arc::clone(&session.load_started),
            session.request.clone(),
            session.storage.clone(),
        )
    };
    let loading = Arc::clone(&model);
    tauri::async_runtime::spawn_blocking(move || {
        load_if_pending(&loading, &load_started, &request, &storage, &|stage| {
            let _ = on_stage.send(stage.to_owned());
        });
    })
    .await
    .expect("the load task should not panic");

    let guard = lock(&model);
    match guard.phase() {
        SessionPhase::Ready(review) => Ok(dto::project_snapshot(review.session())),
        SessionPhase::Failed(failure) => Err(failure.into()),
        SessionPhase::Loading { .. } => {
            unreachable!("finish() always leaves the model Ready or Failed")
        }
    }
}

/// Re-reads the settings file, then fetches what every clone it lists has open.
///
/// Runs when Home opens, on `r`, and after an Add or a Remove. Everything Home
/// shows goes to `on_progress` as each batch of repositories lands, so the list
/// fills in as it loads. A trigger that arrives mid-refresh starts nothing and
/// answers with what is already on screen.
///
/// A settings file that cannot be read, or a `gh` that cannot be used, comes
/// back inside the snapshot rather than as a command error, because the header
/// and footer stay on screen either way.
///
/// # Errors
///
/// Returns a failure when the task doing the fetching does not finish.
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub async fn refresh_home(
    state: tauri::State<'_, AppRoot>,
    on_progress: Channel<dto::HomeSnapshotDto>,
) -> Result<dto::HomeSnapshotDto, dto::SessionFailureDto> {
    let root = state.inner().clone();
    off_the_ui_thread(move || {
        on_settings_file(&root, |root, settings_path| {
            let database_path = store::default_database_path()
                .expect("HOME is set, since the settings path just resolved from it");
            refresh_home_on_model(root, settings_path, &database_path, &|shown| {
                let _ = on_progress.send(shown);
            });
        })
    })
    .await
}

/// Moves the cursor one row through the list, across the groups.
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub fn move_home_cursor(
    state: tauri::State<'_, AppRoot>,
    move_to: dto::CursorMoveDto,
) -> dto::HomeSnapshotDto {
    move_home_cursor_on_model(&state, move_to)
}

/// Adds the folders the reviewer picked, writing the file once and reading it
/// back.
///
/// A folder that is not a clone of a GitHub repository is refused with its
/// reason while the rest proceed, and one already listed is ignored. The pull
/// requests of a repository just added arrive with the refresh the caller runs
/// next, not with this.
///
/// # Errors
///
/// Returns a failure when the task doing the work does not finish.
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub async fn add_repositories(
    state: tauri::State<'_, AppRoot>,
    folders: Vec<String>,
) -> Result<dto::HomeSnapshotDto, dto::SessionFailureDto> {
    let root = state.inner().clone();
    let folders = folders.into_iter().map(PathBuf::from).collect::<Vec<_>>();
    off_the_ui_thread(move || {
        on_settings_file(&root, |root, path| {
            add_repositories_on_model(&root.home, path, &folders);
        })
    })
    .await
}

/// Drops one configured clone, writing the file and reading it back.
///
/// The list is left as it stands otherwise. It is the refresh the caller runs
/// next that asks GitHub about what remains.
///
/// # Errors
///
/// Returns a failure when the task doing the work does not finish.
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub async fn remove_repository(
    state: tauri::State<'_, AppRoot>,
    path: String,
) -> Result<dto::HomeSnapshotDto, dto::SessionFailureDto> {
    let root = state.inner().clone();
    let path = PathBuf::from(path);
    off_the_ui_thread(move || {
        on_settings_file(&root, |root, settings_path| {
            remove_repository_on_model(&root.home, settings_path, &path);
        })
    })
    .await
}

/// Opens or closes the Repositories footer.
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub fn toggle_repositories_footer(state: tauri::State<'_, AppRoot>) -> dto::HomeSnapshotDto {
    lock(&state.home.model).toggle_footer();
    shown_home(&state)
}

/// Which screen the window shows, and the Session it is holding.
///
/// Asked once before anything is rendered, and again after every navigation. A
/// Session carries its request's own description, which is all the loading
/// screen can say before the load reaches the pull request itself.
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub fn describe_window(state: tauri::State<'_, AppRoot>) -> dto::WindowDto {
    describe(&lock(&state.window))
}

/// What the window shows now, from a window already locked.
fn describe(window: &Window) -> dto::WindowDto {
    let session = window.session.as_ref().map(|session| dto::OpenSessionDto {
        description: session.request.description(),
        row_identity: window
            .slot
            .session()
            .and_then(app::OpenSession::row_identity),
    });
    match window.slot.showing() {
        Showing::Home => dto::WindowDto::Home { alive: session },
        Showing::Session => dto::WindowDto::Session {
            session: session.expect("a window showing a Session is holding one"),
        },
    }
}

/// Opens the pull request a Home row names, replacing whatever Session was
/// alive.
///
/// # Errors
///
/// Returns a failure when no configured clone resolves to `repository`, which
/// leaves the Session that was alive exactly as it was.
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub fn open_row(
    state: tauri::State<'_, AppRoot>,
    repository: String,
    number: u32,
) -> Result<dto::WindowDto, dto::SessionFailureDto> {
    open_row_on_root(&state, &repository, u64::from(number))
}

fn open_row_on_root(
    root: &AppRoot,
    repository: &str,
    number: u64,
) -> Result<dto::WindowDto, dto::SessionFailureDto> {
    // Looked up before the window is touched, so a row that cannot be opened
    // costs the reviewer nothing of the Session they were reading.
    let clone_root = configured_clone(&root.home, repository)?;
    let mut window = lock(&root.window);
    let pull_request = PullRequestId {
        repository: repository.to_owned(),
        number,
    };
    match window.open(pull_request, clone_root) {
        Opened::Returned | Opened::Loading => Ok(describe(&window)),
        Opened::Refused => Err(dto::command_failure(
            "this window has no Home to open a row from",
        )),
    }
}

/// The clone to open a row's pull request out of.
///
/// The first in settings order whose remote names `repository`, because two
/// checkouts of one repository reach the same pull request either way.
fn configured_clone(
    home: &ManagedHome,
    repository: &str,
) -> Result<PathBuf, dto::SessionFailureDto> {
    lock(&home.model)
        .repositories()
        .iter()
        .find_map(|entry| match &entry.outcome {
            RepositoryOutcome::Valid { root, slug } => {
                slug.eq_ignore_ascii_case(repository).then(|| root.clone())
            }
            RepositoryOutcome::Failed { .. } => None,
        })
        .ok_or_else(|| {
            dto::command_failure(format!("Home has no configured clone of {repository}"))
        })
}

/// Shows Home again, leaving the Session alive behind it.
///
/// # Errors
///
/// Returns a failure for a Session the command line opened, which has no Home
/// behind it to go back to.
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub fn return_to_home(
    state: tauri::State<'_, AppRoot>,
) -> Result<dto::WindowDto, dto::SessionFailureDto> {
    return_to_home_on_root(&state)
}

fn return_to_home_on_root(root: &AppRoot) -> Result<dto::WindowDto, dto::SessionFailureDto> {
    let mut window = lock(&root.window);
    if !window.slot.back_to_home() {
        return Err(dto::command_failure(
            "this session has no Home to go back to",
        ));
    }
    Ok(describe(&window))
}

/// Shows the Session alive behind Home again, exactly as it was left.
///
/// # Errors
///
/// Returns a failure when no Session is alive, which the header slot's own
/// absence already keeps a reviewer from asking for.
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub fn return_to_session(
    state: tauri::State<'_, AppRoot>,
) -> Result<dto::WindowDto, dto::SessionFailureDto> {
    return_to_session_on_root(&state)
}

fn return_to_session_on_root(root: &AppRoot) -> Result<dto::WindowDto, dto::SessionFailureDto> {
    let mut window = lock(&root.window);
    if !window.slot.return_to_session() {
        return Err(dto::command_failure("no session is open"));
    }
    Ok(describe(&window))
}

/// The model of the Session this window holds, for the commands that need one.
///
/// Absent before a row has been opened, where the frontend never calls them, so
/// a call that arrives anyway is answered rather than assumed away. Handed out
/// as its own handle so no command holds the window's lock while it works.
fn session_model(
    state: &tauri::State<'_, AppRoot>,
) -> Result<Arc<Mutex<app::SessionModel>>, dto::SessionFailureDto> {
    lock(&state.window)
        .session
        .as_ref()
        .map(|session| Arc::clone(&session.model))
        .ok_or_else(|| dto::command_failure("no session is open"))
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
    state: tauri::State<'_, AppRoot>,
    index: u32,
) -> Result<dto::FileDetailDto, dto::SessionFailureDto> {
    let model = session_model(&state)?;
    select_file_on_model(&model, index as usize)
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
    state: tauri::State<'_, AppRoot>,
) -> Result<dto::SidebarDto, dto::SessionFailureDto> {
    let model = session_model(&state)?;
    toggle_viewed_on_model(&model)
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
    state: tauri::State<'_, AppRoot>,
    file_index: u32,
    start: u32,
    end: u32,
    body: String,
) -> Result<dto::DraftEditOutcomeDto, dto::SessionFailureDto> {
    let model = session_model(&state)?;
    edit_draft_on_model(
        &model,
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
    state: tauri::State<'_, AppRoot>,
    file_index: u32,
    row: u32,
) -> Result<dto::DraftsDto, dto::SessionFailureDto> {
    let model = session_model(&state)?;
    discard_draft_on_model(&model, file_index as usize, row as usize)
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
    state: tauri::State<'_, AppRoot>,
    file_index: u32,
    path: String,
    side: dto::DiffSideDto,
    line: u32,
    row: u32,
) -> Result<dto::DraftsDto, dto::SessionFailureDto> {
    let model = session_model(&state)?;
    reanchor_draft_on_model(
        &model,
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
    use crate::fake_gh::FakeGh;

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

    /// A Session dropped mid-load leaves its loader running, and the load that
    /// replaces it reaches the same clone. Two `git fetch` runs in one clone
    /// with nothing between them is what the guard exists to stop.
    #[test]
    fn two_session_loads_run_one_after_the_other() {
        let running = std::sync::atomic::AtomicUsize::new(0);
        let overlapped = AtomicBool::new(false);

        thread::scope(|scope| {
            for _ in 0..2 {
                let running = &running;
                let overlapped = &overlapped;
                scope.spawn(move || {
                    let model = Mutex::new(app::SessionModel::loading("test"));
                    run_load(
                        &model,
                        &SessionRequest::Demo,
                        &ReviewStorage::Disabled,
                        &|_stage| {
                            if running.fetch_add(1, Ordering::AcqRel) > 0 {
                                overlapped.store(true, Ordering::Release);
                            }
                            thread::sleep(std::time::Duration::from_millis(25));
                            running.fetch_sub(1, Ordering::AcqRel);
                        },
                    );
                });
            }
        });

        assert!(
            !overlapped.load(Ordering::Acquire),
            "two loads were in flight at once",
        );
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
    fn a_session_launch_is_described_by_its_own_request() {
        let request = SessionRequest::LocalComparison {
            repository: Path::new("/tmp/repository").to_path_buf(),
            base: "main".to_owned(),
            head: "feature".to_owned(),
        };
        let root = command_line_root(request.clone());

        assert_eq!(
            describe(&lock(&root.window)),
            dto::WindowDto::Session {
                session: dto::OpenSessionDto {
                    description: request.description(),
                    row_identity: None,
                },
            },
        );
    }

    #[test]
    fn a_launch_with_no_session_is_described_as_home() {
        let root = home_root(ManagedHome::new(github::GithubClient::default()));

        assert_eq!(
            describe(&lock(&root.window)),
            dto::WindowDto::Home { alive: None },
        );
    }

    /// A window opened on Home, listing whatever `home` was configured with.
    fn home_root(home: ManagedHome) -> AppRoot {
        AppRoot {
            home,
            window: Arc::new(Mutex::new(Window::home())),
        }
    }

    /// A window the command line opened straight into `request`'s Session.
    fn command_line_root(request: SessionRequest) -> AppRoot {
        AppRoot {
            home: ManagedHome::new(github::GithubClient::default()),
            window: Arc::new(Mutex::new(Window::command_line(
                request,
                ReviewStorage::Disabled,
            ))),
        }
    }

    /// A window on Home with `clone` configured, ready to open a row from it.
    ///
    /// Configured through Add rather than a refresh, because opening a row asks
    /// nothing of GitHub. The load itself is deferred until `open_session`.
    fn home_root_listing(clone: &TempDir, settings_path: &Path) -> AppRoot {
        let home = home_model();
        add_repositories_on_model(&home, settings_path, &[clone.path().to_path_buf()]);
        home_root(home)
    }

    /// Writes `stage` on the Session the window holds.
    ///
    /// A Session kept alive keeps whatever it had reached; one loaded in its
    /// place starts over, which is how a test tells the two apart without
    /// running a load.
    fn mark_session(root: &AppRoot, stage: &str) {
        let window = lock(&root.window);
        let session = window.session.as_ref().expect("a session is open");
        assert!(
            lock(&session.model).set_stage(stage),
            "the mark should take"
        );
    }

    /// How far the Session the window holds has got.
    fn session_stage(root: &AppRoot) -> String {
        let window = lock(&root.window);
        let session = window.session.as_ref().expect("a session is open");
        let guard = lock(&session.model);
        match guard.phase() {
            SessionPhase::Loading { stage, .. } => stage.clone(),
            SessionPhase::Ready(_) | SessionPhase::Failed(_) => {
                panic!("a row's load is deferred until open_session")
            }
        }
    }

    /// What the window is holding, as the request it was opened with.
    fn open_request(root: &AppRoot) -> SessionRequest {
        lock(&root.window)
            .session
            .as_ref()
            .expect("a session is open")
            .request
            .clone()
    }

    #[test]
    fn opening_a_row_opens_its_pull_request_out_of_the_clone_it_is_configured_in() {
        let clone = clone_of("acme/widgets");
        let directory = TempDir::new().unwrap();
        let settings_path = directory.path().join("settings.toml");
        let root = home_root_listing(&clone, &settings_path);

        let shown = open_row_on_root(&root, "acme/widgets", 412).expect("the clone is configured");

        assert_eq!(
            shown,
            dto::WindowDto::Session {
                session: dto::OpenSessionDto {
                    description: "pull request #412".to_owned(),
                    row_identity: Some("acme/widgets#412".to_owned()),
                },
            },
        );
        assert_eq!(
            open_request(&root),
            SessionRequest::PullRequest {
                repository: clone.path().canonicalize().unwrap(),
                selector: github::PullRequestSelector::Number(412),
            },
        );
        assert_eq!(
            lock(&root.window)
                .session
                .as_ref()
                .expect("a session is open")
                .storage,
            ReviewStorage::Default,
            "a row's drafts are kept, so it opens on the default storage",
        );
    }

    /// The whole point of keeping one Session alive. Coming back to it costs no
    /// load at all.
    #[test]
    fn opening_the_row_already_alive_shows_the_same_session_again() {
        let clone = clone_of("acme/widgets");
        let directory = TempDir::new().unwrap();
        let settings_path = directory.path().join("settings.toml");
        let root = home_root_listing(&clone, &settings_path);
        open_row_on_root(&root, "acme/widgets", 412).expect("the clone is configured");
        mark_session(&root, "half way through");
        return_to_home_on_root(&root).expect("there is a Home behind it");

        open_row_on_root(&root, "acme/widgets", 412).expect("the clone is configured");

        assert_eq!(session_stage(&root), "half way through");
    }

    #[test]
    fn opening_another_row_replaces_the_session_that_was_alive() {
        let clone = clone_of("acme/widgets");
        let directory = TempDir::new().unwrap();
        let settings_path = directory.path().join("settings.toml");
        let root = home_root_listing(&clone, &settings_path);
        open_row_on_root(&root, "acme/widgets", 412).expect("the clone is configured");
        mark_session(&root, "half way through");

        open_row_on_root(&root, "acme/widgets", 398).expect("the clone is configured");

        assert_eq!(
            session_stage(&root),
            "Starting",
            "the Session that was alive was dropped, not carried over",
        );
        assert_eq!(
            open_request(&root),
            SessionRequest::PullRequest {
                repository: clone.path().canonicalize().unwrap(),
                selector: github::PullRequestSelector::Number(398),
            },
        );
    }

    #[test]
    fn going_back_shows_home_and_reports_the_session_still_alive_behind_it() {
        let clone = clone_of("acme/widgets");
        let directory = TempDir::new().unwrap();
        let settings_path = directory.path().join("settings.toml");
        let root = home_root_listing(&clone, &settings_path);
        open_row_on_root(&root, "acme/widgets", 412).expect("the clone is configured");
        mark_session(&root, "half way through");

        let shown = return_to_home_on_root(&root).expect("there is a Home behind it");

        assert_eq!(
            shown,
            dto::WindowDto::Home {
                alive: Some(dto::OpenSessionDto {
                    description: "pull request #412".to_owned(),
                    row_identity: Some("acme/widgets#412".to_owned()),
                }),
            },
        );
        assert_eq!(
            session_stage(&root),
            "half way through",
            "back is not a close",
        );
        assert_eq!(describe(&lock(&root.window)), shown);
    }

    /// The header slot is the way back to a Session whose pull request has no
    /// row in the list at all.
    #[test]
    fn the_header_slot_shows_the_session_alive_behind_home_again() {
        let clone = clone_of("acme/widgets");
        let directory = TempDir::new().unwrap();
        let settings_path = directory.path().join("settings.toml");
        let root = home_root_listing(&clone, &settings_path);
        let opened = open_row_on_root(&root, "acme/widgets", 412).expect("the clone is configured");
        return_to_home_on_root(&root).expect("there is a Home behind it");

        let shown = return_to_session_on_root(&root).expect("a session is alive");

        assert_eq!(shown, opened);
    }

    #[test]
    fn returning_to_a_session_before_any_row_was_opened_says_there_is_none() {
        let root = home_root(home_model());

        let refused = return_to_session_on_root(&root).expect_err("no session is alive");

        assert_eq!(refused.summary, "no session is open");
    }

    /// Only Home lists a row, and a window the command line opened never shows
    /// one, so a row arriving here is answered rather than acted on.
    #[test]
    fn a_window_the_command_line_opened_refuses_to_open_a_row() {
        let clone = clone_of("acme/widgets");
        let directory = TempDir::new().unwrap();
        let settings_path = directory.path().join("settings.toml");
        let root = command_line_root(SessionRequest::Demo);
        add_repositories_on_model(&root.home, &settings_path, &[clone.path().to_path_buf()]);

        let refused =
            open_row_on_root(&root, "acme/widgets", 412).expect_err("there is no Home here");

        assert_eq!(
            refused.summary,
            "this window has no Home to open a row from"
        );
        assert_eq!(
            describe(&lock(&root.window)),
            dto::WindowDto::Session {
                session: dto::OpenSessionDto {
                    description: SessionRequest::Demo.description(),
                    row_identity: None,
                },
            },
            "the Session it was opened with is still the one it holds",
        );
    }

    /// Two checkouts of one repository reach the same pull request, so the row
    /// opens out of whichever the reviewer configured first.
    #[test]
    fn a_row_opens_out_of_the_first_clone_of_its_repository_in_settings_order() {
        let first = clone_of("acme/widgets");
        let second = clone_of("acme/widgets");
        let directory = TempDir::new().unwrap();
        let settings_path = directory.path().join("settings.toml");
        let home = home_model();
        add_repositories_on_model(
            &home,
            &settings_path,
            &[first.path().to_path_buf(), second.path().to_path_buf()],
        );
        let root = home_root(home);

        open_row_on_root(&root, "acme/widgets", 412).expect("both clones are configured");

        assert_eq!(
            open_request(&root),
            SessionRequest::PullRequest {
                repository: first.path().canonicalize().unwrap(),
                selector: github::PullRequestSelector::Number(412),
            },
        );
    }

    /// GitHub compares two repository names without regard to case, so a row
    /// that came back cased differently still finds its clone.
    #[test]
    fn a_row_cased_differently_from_its_clone_still_opens() {
        let clone = clone_of("ACME/Widgets");
        let directory = TempDir::new().unwrap();
        let settings_path = directory.path().join("settings.toml");
        let root = home_root_listing(&clone, &settings_path);

        open_row_on_root(&root, "acme/widgets", 412).expect("the clone is configured");

        assert_eq!(
            open_request(&root),
            SessionRequest::PullRequest {
                repository: clone.path().canonicalize().unwrap(),
                selector: github::PullRequestSelector::Number(412),
            },
        );
    }

    /// The mark is the only thing on a row that says which Session is alive, so
    /// it is decided where two repository names are compared properly.
    #[test]
    fn the_row_of_the_alive_session_comes_back_marked_however_it_is_cased() {
        let gh = FakeGh::new();
        gh.answer_graphql(&recorded_search("acme/widgets"));
        let clone = clone_of("acme/widgets");
        let directory = TempDir::new().unwrap();
        let settings_path = settings_listing(&directory, &[&clone]);
        let root = home_root(home_with(&gh));
        refresh_home_on_model(
            &root,
            &settings_path,
            &no_drafts_database(),
            &|_progress| {},
        );
        assert!(
            shown_home(&root).groups[0]
                .rows
                .iter()
                .all(|row| !row.is_alive),
            "nothing is alive before a row is opened",
        );

        open_row_on_root(&root, "ACME/Widgets", 412).expect("the clone is configured");
        return_to_home_on_root(&root).expect("there is a Home behind it");

        let marked = shown_home(&root).groups[0]
            .rows
            .iter()
            .filter(|row| row.is_alive)
            .map(|row| row.identity.clone())
            .collect::<Vec<_>>();
        assert_eq!(marked, ["acme/widgets#412"]);
    }

    #[test]
    fn a_session_the_command_line_opened_has_no_home_to_go_back_to() {
        let root = command_line_root(SessionRequest::Demo);

        let refused = return_to_home_on_root(&root).expect_err("there is no Home behind it");

        assert_eq!(refused.summary, "this session has no Home to go back to");
    }

    /// The row's repository can be dropped from the settings file between the
    /// refresh that listed it and the Enter that opens it.
    #[test]
    fn opening_a_row_of_a_repository_no_longer_configured_says_which_one() {
        let root = home_root(home_model());

        let refused =
            open_row_on_root(&root, "acme/widgets", 412).expect_err("nothing is configured");

        assert_eq!(
            refused.summary,
            "Home has no configured clone of acme/widgets"
        );
        assert!(
            lock(&root.window).session.is_none(),
            "a row that could not be opened drops nothing",
        );
    }

    /// A refusal must not cost the reviewer the Session they were reading.
    #[test]
    fn a_row_that_could_not_be_opened_leaves_the_session_alive() {
        let clone = clone_of("acme/widgets");
        let directory = TempDir::new().unwrap();
        let settings_path = directory.path().join("settings.toml");
        let root = home_root_listing(&clone, &settings_path);
        open_row_on_root(&root, "acme/widgets", 412).expect("the clone is configured");
        mark_session(&root, "half way through");
        return_to_home_on_root(&root).expect("there is a Home behind it");

        open_row_on_root(&root, "acme/billing", 7).expect_err("that clone is not configured");

        assert_eq!(session_stage(&root), "half way through");
        assert_eq!(
            describe(&lock(&root.window)),
            dto::WindowDto::Home {
                alive: Some(dto::OpenSessionDto {
                    description: "pull request #412".to_owned(),
                    row_identity: Some("acme/widgets#412".to_owned()),
                }),
            },
        );
    }

    /// A clone whose `origin` points at GitHub, which is what Home configures.
    fn clone_of(slug: &str) -> TempDir {
        let directory = TempDir::new().unwrap();
        git(directory.path(), ["init", "--quiet"]);
        git(
            directory.path(),
            [
                "remote",
                "add",
                "origin",
                &format!("https://github.com/{slug}.git"),
            ],
        );
        directory
    }

    /// Home with a `gh` that is never run, for the actions that never reach it.
    fn home_model() -> ManagedHome {
        ManagedHome::new(github::GithubClient::new("gh-that-is-never-run"))
    }

    /// What Home shows once an action has finished.
    ///
    /// Wrapped in a window of its own, because these tests exercise the list
    /// rather than the Session that can be alive behind it.
    fn snapshot(home: &ManagedHome) -> dto::HomeSnapshotDto {
        shown_home(&home_root(home.clone()))
    }

    /// The repository paths the settings file now holds.
    fn listed(settings_path: &Path) -> Vec<std::path::PathBuf> {
        settings::load(settings_path).unwrap().repositories
    }

    /// A database path guaranteed not to exist, for tests that are not
    /// exercising Drafts and want a refresh to see none.
    fn no_drafts_database() -> PathBuf {
        TempDir::new().unwrap().path().join("review-data.sqlite3")
    }

    /// A refresh with nothing listening to its progress.
    fn refresh(home: &ManagedHome, settings_path: &Path) {
        refresh_home_on_model(
            &home_root(home.clone()),
            settings_path,
            &no_drafts_database(),
            &|_progress| {},
        );
    }

    /// A settings file listing `clones`.
    fn settings_listing(directory: &TempDir, clones: &[&TempDir]) -> PathBuf {
        let listed = clones
            .iter()
            .map(|clone| format!("{:?}", clone.path().display().to_string()))
            .collect::<Vec<_>>()
            .join(", ");
        let path = directory.path().join("settings.toml");
        std::fs::write(&path, format!("repositories = [{listed}]\n")).unwrap();
        path
    }

    /// One recorded search answering for `slug`, with two rows to review.
    fn recorded_search(slug: &str) -> String {
        format!(
            r#"{{"data":{{
                "viewer":{{"login":"braidonw"}},
                "rateLimit":{{"cost":1,"remaining":4999,"resetAt":"2026-09-01T13:00:00Z"}},
                "toReview":{{"issueCount":2,"pageInfo":{{"hasNextPage":false,"endCursor":null}},"nodes":[
                  {{"number":412,"title":"Retry webhook deliveries","url":"https://github.com/{slug}/pull/412",
                   "isDraft":false,"updatedAt":"2026-09-01T12:34:56Z","headRefOid":"abc123",
                   "repository":{{"nameWithOwner":"{slug}"}},"author":{{"login":"mlee"}},
                   "viewerLatestReview":null,"statusCheckRollup":{{"state":"SUCCESS"}}}},
                  {{"number":398,"title":"Split the renderer","url":"https://github.com/{slug}/pull/398",
                   "isDraft":false,"updatedAt":"2026-08-30T09:00:00Z","headRefOid":"def456",
                   "repository":{{"nameWithOwner":"{slug}"}},"author":{{"login":"priya"}},
                   "viewerLatestReview":null,"statusCheckRollup":null}}
                ]}},
                "authored":{{"issueCount":0,"pageInfo":{{"hasNextPage":false,"endCursor":null}},"nodes":[]}}
            }}}}"#,
        )
    }

    /// Home reading `settings_path`, with a `gh` a test has set up.
    fn home_with(gh: &FakeGh) -> ManagedHome {
        ManagedHome::new(gh.client())
    }

    #[test]
    fn a_refresh_lists_the_pull_requests_it_fetched() {
        let gh = FakeGh::new();
        gh.answer_graphql(&recorded_search("acme/widgets"));
        let clone = clone_of("acme/widgets");
        let directory = TempDir::new().unwrap();
        let settings_path = settings_listing(&directory, &[&clone]);
        let home = home_with(&gh);

        refresh(&home, &settings_path);

        let shown = snapshot(&home);
        assert_eq!(
            shown.count_line.as_deref(),
            Some("2 pull requests across 1 repository"),
        );
        let to_review = &shown.groups[0];
        assert_eq!(to_review.title, "To review");
        assert_eq!(to_review.count, 2);
        assert_eq!(to_review.rows[0].identity, "acme/widgets#412");
        assert_eq!(to_review.rows[0].title, "Retry webhook deliveries");
        assert_eq!(to_review.rows[0].author.as_deref(), Some("mlee"));
        assert_eq!(
            to_review.rows[0]
                .check_status
                .as_ref()
                .expect("the head has checks")
                .label,
            "checks passing",
        );
        assert!(
            to_review.rows[1].check_status.is_none(),
            "a head with no checks leaves the column empty",
        );
        assert!(shown.failed_repositories.is_empty());
    }

    /// Everything a refresh reported, in the order it said it.
    fn reported_by(home: &ManagedHome, settings_path: &Path) -> Vec<dto::HomeSnapshotDto> {
        let reported = Mutex::new(Vec::new());
        refresh_home_on_model(
            &home_root(home.clone()),
            settings_path,
            &no_drafts_database(),
            &|shown| {
                reported.lock().unwrap().push(shown);
            },
        );
        reported.into_inner().unwrap()
    }

    #[test]
    fn a_refresh_counts_off_the_repositories_and_ends_with_the_time_it_settled() {
        let gh = FakeGh::new();
        gh.answer_graphql(&recorded_search("acme/widgets"));
        let clone = clone_of("acme/widgets");
        let directory = TempDir::new().unwrap();
        let settings_path = settings_listing(&directory, &[&clone]);
        let home = home_with(&gh);

        let reported = reported_by(&home, &settings_path);

        let states = reported
            .iter()
            .map(|shown| shown.refresh.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            states.first(),
            Some(&dto::RefreshStateDto::Refreshing { done: 0, total: 0 }),
            "the total is unknown until the settings file has been read",
        );
        assert!(
            states.contains(&dto::RefreshStateDto::Refreshing { done: 1, total: 1 }),
            "every repository is counted off: {states:?}",
        );
        assert!(
            matches!(states.last(), Some(dto::RefreshStateDto::Refreshed { .. })),
            "the last thing reported is the settled stamp: {states:?}",
        );
    }

    /// A reviewer watching a slow organisation sees the rest of the list before
    /// it answers, so every batch carries what Home shows by then.
    #[test]
    fn a_refresh_reports_the_rows_it_has_as_each_batch_lands() {
        let gh = FakeGh::new();
        gh.answer_graphql(&recorded_search("acme/widgets"));
        let clone = clone_of("acme/widgets");
        let directory = TempDir::new().unwrap();
        let settings_path = settings_listing(&directory, &[&clone]);
        let home = home_with(&gh);

        let reported = reported_by(&home, &settings_path);

        let counted = reported
            .iter()
            .map(|shown| shown.groups[0].rows.len())
            .collect::<Vec<_>>();
        assert_eq!(
            counted,
            [0, 0, 2, 2],
            "the rows arrive with the batch that fetched them",
        );
    }

    /// Two refreshes never race, and the one that lost was not queued behind
    /// the one that won.
    #[test]
    fn a_refresh_that_arrives_while_the_settings_guard_is_held_starts_nothing() {
        let gh = FakeGh::new();
        gh.answer_graphql(&recorded_search("acme/widgets"));
        let clone = clone_of("acme/widgets");
        let directory = TempDir::new().unwrap();
        let settings_path = settings_listing(&directory, &[&clone]);
        let home = home_with(&gh);
        refresh(&home, &settings_path);
        let settled = snapshot(&home);

        let held = lock(&home.settings_action);
        let ignored = on_settings_file(&home_root(home.clone()), |root, settings_path| {
            refresh(&root.home, settings_path);
        });
        drop(held);

        assert_eq!(ignored.refresh, settled.refresh, "the stamp is left alone");
        assert_eq!(
            ignored.groups[0].rows.len(),
            2,
            "the rows on screen are still on screen",
        );
        assert_eq!(ignored.count_line, settled.count_line);
    }

    /// A refresh that stopped part way must not leave a stamp counting off for
    /// ever, with nothing left running to finish it.
    #[test]
    fn a_refresh_that_panicked_ends_as_failed_rather_than_running_for_ever() {
        let gh = FakeGh::new();
        gh.answer_graphql(&recorded_search("acme/widgets"));
        let clone = clone_of("acme/widgets");
        let directory = TempDir::new().unwrap();
        let settings_path = settings_listing(&directory, &[&clone]);
        let home = home_with(&gh);
        let reports = std::sync::atomic::AtomicUsize::new(0);

        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            refresh_home_on_model(
                &home_root(home.clone()),
                &settings_path,
                &no_drafts_database(),
                &|_shown| {
                    assert!(
                        reports.fetch_add(1, Ordering::AcqRel) < 2,
                        "the refresh gave up here"
                    );
                },
            );
        }));

        assert!(panicked.is_err(), "the refresh was stopped by a panic");
        let shown = snapshot(&home);
        assert_eq!(shown.refresh, dto::RefreshStateDto::Failed);
        assert_eq!(
            shown.failure.expect("an abandoned refresh says so").summary,
            "Home could not finish the refresh",
        );
    }

    /// The next trigger has to be able to start, whatever became of the last.
    #[test]
    fn a_refresh_after_one_that_panicked_still_runs() {
        let gh = FakeGh::new();
        gh.answer_graphql(&recorded_search("acme/widgets"));
        let clone = clone_of("acme/widgets");
        let directory = TempDir::new().unwrap();
        let settings_path = settings_listing(&directory, &[&clone]);
        let home = home_with(&gh);
        let _panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            refresh_home_on_model(
                &home_root(home.clone()),
                &settings_path,
                &no_drafts_database(),
                &|_shown| panic!("gave up"),
            );
        }));

        refresh(&home, &settings_path);

        let shown = snapshot(&home);
        assert!(matches!(
            shown.refresh,
            dto::RefreshStateDto::Refreshed { .. }
        ));
        assert_eq!(shown.groups[0].rows.len(), 2);
    }

    #[test]
    fn a_gh_that_cannot_be_used_stops_the_refresh_and_says_what_to_fix() {
        let gh = FakeGh::new();
        gh.refuse_authentication();
        let clone = clone_of("acme/widgets");
        let directory = TempDir::new().unwrap();
        let settings_path = settings_listing(&directory, &[&clone]);
        let home = home_with(&gh);

        refresh(&home, &settings_path);

        let shown = snapshot(&home);
        let failure = shown
            .failure
            .expect("the preflight failure replaces the list");
        assert_eq!(failure.summary, "Home could not use the GitHub CLI");
        assert!(failure.remediation.unwrap().contains("gh auth login"));
        assert_eq!(shown.refresh, dto::RefreshStateDto::Failed);
        assert_eq!(
            shown.repositories.len(),
            1,
            "an unusable gh says nothing about the configured clones",
        );
    }

    /// Fixing `gh` and pressing `r` is all it takes to get the list back.
    #[test]
    fn a_refresh_after_gh_is_fixed_restores_the_list() {
        let gh = FakeGh::new();
        gh.refuse_authentication();
        gh.answer_graphql(&recorded_search("acme/widgets"));
        let clone = clone_of("acme/widgets");
        let directory = TempDir::new().unwrap();
        let settings_path = settings_listing(&directory, &[&clone]);
        let home = home_with(&gh);
        refresh(&home, &settings_path);
        assert!(snapshot(&home).failure.is_some());

        gh.allow_authentication();
        refresh(&home, &settings_path);

        let shown = snapshot(&home);
        assert!(shown.failure.is_none());
        assert_eq!(shown.groups[0].count, 2);
    }

    #[test]
    fn a_repository_that_could_not_be_fetched_shows_above_the_list_and_in_the_footer() {
        let gh = FakeGh::new();
        let clone = clone_of("acme/widgets");
        let directory = TempDir::new().unwrap();
        let settings_path = settings_listing(&directory, &[&clone]);
        let home = home_with(&gh);

        refresh(&home, &settings_path);

        let shown = snapshot(&home);
        assert_eq!(shown.failed_repositories.len(), 1);
        assert_eq!(shown.failed_repositories[0].slug, "acme/widgets");
        assert_eq!(
            shown.failed_repositories[0].path,
            clone.path().display().to_string()
        );
        assert!(!shown.failed_repositories[0].reason.is_empty());
        assert!(shown.repositories[0].failure.is_some());
        assert_eq!(
            shown.refresh,
            dto::RefreshStateDto::Failed,
            "nothing loaded at all",
        );
    }

    #[test]
    fn the_cursor_moves_down_and_up_over_the_rows_a_refresh_listed() {
        let gh = FakeGh::new();
        gh.answer_graphql(&recorded_search("acme/widgets"));
        let clone = clone_of("acme/widgets");
        let directory = TempDir::new().unwrap();
        let settings_path = settings_listing(&directory, &[&clone]);
        let home = home_with(&gh);
        refresh(&home, &settings_path);
        assert_eq!(snapshot(&home).cursor, 0);

        assert_eq!(
            move_home_cursor_on_model(&home_root(home.clone()), dto::CursorMoveDto::Down).cursor,
            1,
        );
        assert_eq!(
            move_home_cursor_on_model(&home_root(home.clone()), dto::CursorMoveDto::Up).cursor,
            0
        );
    }

    #[test]
    fn refreshing_home_reads_the_settings_file_every_time() {
        let clone = clone_of("acme/widgets");
        let directory = TempDir::new().unwrap();
        let settings_path = directory.path().join("settings.toml");
        let home = home_model();

        refresh(&home, &settings_path);
        assert!(snapshot(&home).repositories.is_empty());

        std::fs::write(
            &settings_path,
            format!(
                "repositories = [{:?}]\n",
                clone.path().display().to_string()
            ),
        )
        .unwrap();
        refresh(&home, &settings_path);

        let shown = snapshot(&home);
        assert_eq!(shown.repositories.len(), 1, "a hand edit is picked up");
        assert_eq!(shown.repositories[0].slug.as_deref(), Some("acme/widgets"));
    }

    #[test]
    fn adding_a_clone_writes_the_file_and_lists_it() {
        let clone = clone_of("acme/widgets");
        let directory = TempDir::new().unwrap();
        let settings_path = directory.path().join("settings.toml");
        let home = home_model();

        add_repositories_on_model(&home, &settings_path, &[clone.path().to_path_buf()]);

        assert_eq!(listed(&settings_path).len(), 1);
        assert_eq!(
            snapshot(&home).repositories[0].slug.as_deref(),
            Some("acme/widgets"),
        );
    }

    #[test]
    fn adding_a_folder_that_is_not_a_clone_refuses_it_and_writes_nothing() {
        let not_a_clone = TempDir::new().unwrap();
        let directory = TempDir::new().unwrap();
        let settings_path = directory.path().join("settings.toml");
        let home = home_model();

        add_repositories_on_model(&home, &settings_path, &[not_a_clone.path().to_path_buf()]);

        assert!(
            !settings_path.exists(),
            "nothing was accepted, so the file is never touched",
        );
        let shown = snapshot(&home);
        assert_eq!(shown.refusals.len(), 1);
        assert_eq!(shown.refusals[0].reason, "not a Git repository");
    }

    #[test]
    fn removing_a_repository_writes_the_file_without_it() {
        let first = clone_of("acme/widgets");
        let second = clone_of("acme/billing");
        let directory = TempDir::new().unwrap();
        let settings_path = directory.path().join("settings.toml");
        let home = home_model();
        add_repositories_on_model(
            &home,
            &settings_path,
            &[first.path().to_path_buf(), second.path().to_path_buf()],
        );
        let listed_first = PathBuf::from(&snapshot(&home).repositories[0].path);

        remove_repository_on_model(&home, &settings_path, &listed_first);

        assert_eq!(listed(&settings_path).len(), 1);
        assert_eq!(
            snapshot(&home).repositories[0].slug.as_deref(),
            Some("acme/billing"),
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_settings_file_that_cannot_be_written_is_a_line_above_a_list_that_stays() {
        use std::os::unix::fs::PermissionsExt;

        let clone = clone_of("acme/widgets");
        let directory = TempDir::new().unwrap();
        let settings_path = directory.path().join("settings.toml");
        let home = home_model();
        add_repositories_on_model(&home, &settings_path, &[clone.path().to_path_buf()]);
        // Readable, so the refresh that follows still finds the clone, but not
        // writable, so the second Add cannot record anything.
        std::fs::set_permissions(&settings_path, std::fs::Permissions::from_mode(0o444)).unwrap();

        let second = clone_of("acme/billing");
        add_repositories_on_model(&home, &settings_path, &[second.path().to_path_buf()]);

        let shown = snapshot(&home);
        assert_eq!(
            shown
                .write_failure
                .expect("the write failure should be shown")
                .summary,
            "Home could not save your settings",
        );
        assert!(
            shown.failure.is_none(),
            "a write failure never replaces the list",
        );
        assert_eq!(
            shown.repositories.len(),
            1,
            "the clone that is configured is still listed",
        );
    }

    /// A write replaces the whole file, so writing over one Home could not parse
    /// would drop every repository it never managed to read.
    #[test]
    fn adding_over_a_malformed_settings_file_leaves_the_file_exactly_as_it_was() {
        let clone = clone_of("acme/widgets");
        let directory = TempDir::new().unwrap();
        let settings_path = directory.path().join("settings.toml");
        let malformed = "repositories = [this is not valid toml";
        std::fs::write(&settings_path, malformed).unwrap();
        let home = home_model();
        refresh(&home, &settings_path);

        add_repositories_on_model(&home, &settings_path, &[clone.path().to_path_buf()]);

        assert_eq!(
            std::fs::read_to_string(&settings_path).unwrap(),
            malformed,
            "the file a reviewer has to fix must survive the attempt",
        );
        let shown = snapshot(&home);
        assert_eq!(shown.refusals.len(), 1);
        assert_eq!(shown.refusals[0].path, settings_path.display().to_string());
        assert_eq!(
            shown.refusals[0].reason,
            "fix this file before changing your repositories",
        );
    }

    #[test]
    fn removing_over_a_malformed_settings_file_leaves_the_file_exactly_as_it_was() {
        let directory = TempDir::new().unwrap();
        let settings_path = directory.path().join("settings.toml");
        let malformed = "repositories = [this is not valid toml";
        std::fs::write(&settings_path, malformed).unwrap();
        let home = home_model();
        refresh(&home, &settings_path);

        remove_repository_on_model(&home, &settings_path, Path::new("/Developer/zreview"));

        assert_eq!(std::fs::read_to_string(&settings_path).unwrap(), malformed,);
    }

    /// Two clicks landing together must not read the same list twice and write
    /// back one that still holds the entry the other removed.
    #[test]
    fn two_concurrent_removes_leave_neither_entry_in_the_file() {
        let first = clone_of("acme/widgets");
        let second = clone_of("acme/billing");
        let directory = TempDir::new().unwrap();
        let settings_path = directory.path().join("settings.toml");
        let home = home_model();
        add_repositories_on_model(
            &home,
            &settings_path,
            &[first.path().to_path_buf(), second.path().to_path_buf()],
        );
        let paths = snapshot(&home)
            .repositories
            .iter()
            .map(|entry| PathBuf::from(&entry.path))
            .collect::<Vec<_>>();
        assert_eq!(paths.len(), 2);

        thread::scope(|scope| {
            for path in &paths {
                let home = &home;
                let settings_path = &settings_path;
                scope.spawn(move || remove_repository_on_model(home, settings_path, path));
            }
        });

        assert!(
            listed(&settings_path).is_empty(),
            "one remove read a list the other had already changed",
        );
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
