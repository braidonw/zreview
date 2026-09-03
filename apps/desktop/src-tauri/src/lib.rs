//! The Tauri shell: wires the framework-free `app` crate to a window.

use std::sync::{Arc, Mutex, atomic::AtomicBool};

use app::{HomeModel, SessionModel};
use github::GithubClient;
use session::{ReviewStorage, SessionRequest};

mod commands;
mod drafts;
mod dto;
#[cfg(test)]
mod fake_gh;
mod pull_requests;
mod repositories;

/// What the binary was asked to open.
///
/// Home is the screen with no pull request named. Everything else opens one
/// review sitting straight away, from the command line, and never sees Home.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Launch {
    Home,
    Session {
        request: SessionRequest,
        storage: ReviewStorage,
    },
}

/// The one review sitting this window shows, shared with every command.
pub(crate) struct ManagedSession {
    pub(crate) model: Arc<Mutex<SessionModel>>,
    /// What was asked to be opened, kept so a deferred load knows what to load.
    pub(crate) request: SessionRequest,
    /// Where that request's drafts are persisted.
    pub(crate) storage: ReviewStorage,
    /// Swapped `true` by whichever `open_session` call loads first, so a second
    /// concurrent call (React `StrictMode`'s double mount effect, for one)
    /// waits for that load instead of starting a redundant one of its own.
    pub(crate) load_started: Arc<AtomicBool>,
}

/// Home's state and the guard that orders the actions on its settings file.
///
/// Cloned into whatever thread an action runs on, so both halves travel
/// together and no caller can take one without the other.
#[derive(Clone)]
pub(crate) struct ManagedHome {
    pub(crate) model: Arc<Mutex<HomeModel>>,
    /// Held for the whole of a refresh, an Add, or a Remove.
    ///
    /// Each of those reads the file, decides against what it read, writes, and
    /// reads back. Two of them interleaving would let one write a list the
    /// other had already changed, resurrecting a repository just removed.
    pub(crate) settings_action: Arc<Mutex<()>>,
    /// How a refresh reaches GitHub. Held rather than built per call so a test
    /// can hand Home a `gh` of its own.
    pub(crate) client: GithubClient,
}

impl ManagedHome {
    pub(crate) fn new(client: GithubClient) -> Self {
        Self {
            model: Arc::new(Mutex::new(HomeModel::new())),
            settings_action: Arc::new(Mutex::new(())),
            client,
        }
    }
}

/// Everything the window holds, shared with every command.
///
/// Home and at most one Session. For now a Session is here only when the binary
/// was launched straight into one; Home does not open Sessions yet.
pub(crate) struct AppRoot {
    pub(crate) home: ManagedHome,
    pub(crate) session: Option<ManagedSession>,
}

/// Builds the window and runs the application until it closes.
///
/// # Panics
///
/// Panics if the Tauri application fails to start, or if bindings export fails
/// in a debug build.
pub fn run(launch: Launch) {
    let session = match launch {
        Launch::Home => None,
        Launch::Session { request, storage } => Some(ManagedSession {
            model: Arc::new(Mutex::new(SessionModel::loading(request.description()))),
            request,
            storage,
            load_started: Arc::new(AtomicBool::new(false)),
        }),
    };
    let root = AppRoot {
        home: ManagedHome::new(GithubClient::default()),
        session,
    };
    let specta_builder = commands::specta_builder();

    tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(root)
        .invoke_handler(specta_builder.invoke_handler())
        .setup(move |_app| {
            #[cfg(debug_assertions)]
            specta_builder
                .export(
                    specta_typescript::Typescript::default(),
                    "../src/bindings.ts",
                )
                .expect("failed to export typescript bindings");
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running the tauri application");
}
