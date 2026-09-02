//! The Tauri shell: wires the framework-free `app` crate to a window.

use std::sync::{Arc, Mutex};

use app::SessionModel;
use session::SessionRequest;

mod commands;
mod dto;

/// The one review sitting this window shows, shared with every command.
pub(crate) struct ManagedSession(pub(crate) Arc<Mutex<SessionModel>>);

/// Builds the window and runs the application until it closes.
///
/// # Panics
///
/// Panics if the Tauri application fails to start, or if bindings export fails
/// in a debug build.
pub fn run() {
    let model = Arc::new(Mutex::new(SessionModel::loading(
        SessionRequest::Demo.description(),
    )));
    let specta_builder = commands::specta_builder();

    tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        .manage(ManagedSession(model))
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
