//! Desktop entry point with fast first paint and single-instance windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod attachments;
mod composer;
mod chats;
mod events;
mod instance;
mod markdown;
mod opencode;
mod questions;
mod suggestions;
mod store;
#[cfg(windows)]
mod window;

use std::{error::Error, time::Duration};

slint::include_modules!();

// Show the shell first; open SQLite and Node after the first frame.
fn main() -> Result<(), Box<dyn Error>> {
    // Route second launches to this process for an instant window.
    let directory = std::env::current_dir()?.to_string_lossy().into_owned();
    let port = instance::port_for_dir(&directory);
    if let Some(listener) = instance::try_primary(port) {
        instance::listen(listener);
    } else if instance::notify(port, &directory) {
        return Ok(());
    }
    #[cfg(windows)]
    slint::BackendSelector::new().backend_name("winit".into()).select()?;
    // Build shared state and the first shell before any heavy work.
    let shared = app::init_shared(directory.clone());
    app::create_window(&shared, directory)?;
    // Let the shell paint before loading local chat metadata.
    let deferred = shared.clone();
    slint::Timer::single_shot(Duration::from_millis(32), move || {
        app::ensure_store(deferred);
    });
    // Drain any second-launch requests that arrived during startup.
    slint::Timer::single_shot(Duration::from_millis(200), || {
        app::drain_requests();
    });
    slint::run_event_loop()?;
    Ok(())
}
