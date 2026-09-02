//! The Tauri shell: wires the framework-free `app` crate to a window.

use std::sync::{Arc, Mutex, atomic::AtomicBool};

use app::SessionModel;
use session::{ReviewStorage, SessionRequest};

mod commands;
mod dto;

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

/// Builds the window and runs the application until it closes.
///
/// # Panics
///
/// Panics if the Tauri application fails to start, or if bindings export fails
/// in a debug build.
pub fn run(request: SessionRequest, storage: ReviewStorage) {
    let model = Arc::new(Mutex::new(SessionModel::loading(request.description())));
    let managed = ManagedSession {
        model,
        request,
        storage,
        load_started: Arc::new(AtomicBool::new(false)),
    };
    let specta_builder = commands::specta_builder();

    tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        .manage(managed)
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
