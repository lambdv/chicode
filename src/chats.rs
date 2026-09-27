//! Per-window chat actions sharing one backend.
use crate::{AppWindow, app, store::Chat};
use serde_json::json;
use slint::{ComponentHandle, Model};
use std::{cell::RefCell, rc::Rc};

// Open the first chat, creating one when the sidebar is empty.
pub fn open_first(ui: &AppWindow, shared: &Rc<RefCell<app::Shared>>) {
    let chats = app::show_chats(ui, &shared.borrow()).unwrap_or_default();
    if let Some(chat) = chats.first() {
        app::open(ui, shared, chat);
    } else {
        let created = shared.borrow().store.as_ref().and_then(|store| store.create().ok());
        if let Some(chat) = created {
            app::refresh_all(&shared.borrow());
            app::open(ui, shared, &chat);
        }
    }
}

// Wire sidebar, model, permission, and composer actions for one window.
pub fn wire(ui: &AppWindow, shared: &Rc<RefCell<app::Shared>>) {
    // Every request creates a separate draft, even when other drafts are empty.
    let s = shared.clone();
    let weak = ui.as_weak();
    ui.on_new_chat(move || {
        let ui = weak.unwrap();
        let shared_ref = s.borrow();
        let Some(store) = &shared_ref.store else { return };
        match store.create() {
            Ok(chat) => {
                drop(shared_ref);
                app::refresh_all(&s.borrow());
                app::open(&ui, &s, &chat);
            }
            Err(error) => app::append(&ui, "error", &format!("Cannot create chat: {error}")),
        }
    });
    // Sidebar buttons select a chat by its local ID.
    let s = shared.clone();
    let weak = ui.as_weak();
    ui.on_open_session(move |id| {
        let ui = weak.unwrap();
        if ui.get_active_id() == id && s.borrow().ready.contains(&(id as i64)) {
            return;
        }
        let shared_ref = s.borrow();
        let Some(store) = &shared_ref.store else { return };
        match store.list() {
            Ok(chats) => {
                if let Some(chat) = chats.iter().find(|chat| chat.id == id as i64) {
                    let chat = Chat { id: chat.id, title: chat.title.clone(), session: chat.session.clone(), started: chat.started, project_id: chat.project_id };
                    drop(shared_ref);
                    app::open(&ui, &s, &chat);
                }
            }
            Err(error) => app::append(&ui, "error", &format!("Cannot open chat: {error}")),
        }
    });
    // Persist the displayed order after a session is dragged to another row.
    let s = shared.clone();
    let weak = ui.as_weak();
    ui.on_move_session(move |id, target| {
        let Some(ui) = weak.upgrade() else { return };
        let state = s.borrow();
        let Some(store) = &state.store else { return };
        match store.list() {
            Ok(chats) => {
                let Some(from) = chats.iter().position(|chat| chat.id == id as i64) else { return };
                let target = (target as usize).min(chats.len().saturating_sub(1));
                if from == target { return; }
                let mut ids = chats.into_iter().map(|chat| chat.id).collect::<Vec<_>>();
                let moved = ids.remove(from);
                ids.insert(target, moved);
                if let Err(error) = store.reorder(&ids) {
                    app::append(&ui, "error", &format!("Cannot reorder chats: {error}"));
                    return;
                }
                drop(state);
                app::refresh_all(&s.borrow());
            }
            Err(error) => app::append(&ui, "error", &format!("Cannot reorder chats: {error}")),
        }
    });
    // Hiding a chat never deletes its OpenCode session.
    let s = shared.clone();
    let weak = ui.as_weak();
    ui.on_close_session(move |id| {
        let ui = weak.unwrap();
        let shared_ref = s.borrow();
        let Some(store) = &shared_ref.store else { return };
        if let Err(error) = store.close(id as i64) {
            app::append(&ui, "error", &format!("Cannot close chat: {error}"));
            return;
        }
        app::send(&ui, &shared_ref, json!({ "kind": "close", "id": id }));
        drop(shared_ref);
        s.borrow_mut().ready.remove(&(id as i64));
        app::refresh_all(&s.borrow());
        if ui.get_active_id() == id {
            open_first(&ui, &s);
        }
    });
    // Choose a native folder without blocking the window while the dialog is open.
    let weak = ui.as_weak();
    ui.on_new_project(move || {
        let target_id = weak.upgrade().map(|ui| ui.get_active_id()).unwrap_or(-1);
        std::thread::spawn(move || {
            if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                let _ = slint::invoke_from_event_loop(move || app::add_project(folder, target_id));
            }
        });
    });
    // Changing a project's folder is allowed only before the first prompt.
    let s = shared.clone();
    let weak = ui.as_weak();
    ui.on_select_project(move |project_id| {
        let ui = weak.unwrap();
        let id = ui.get_active_id() as i64;
        let state = s.borrow();
        let Some(store) = &state.store else { return };
        let Ok(projects) = store.projects() else { return };
        let Some(project) = projects.iter().find(|project| project.id == project_id as i64) else { return };
        match store.set_project(id, project.id) {
            Ok(true) => {
                if let Err(error) = store.select_project(project.id) {
                    app::append(&ui, "error", &format!("Cannot save project: {error}"));
                }
                let chat = store.list().ok().and_then(|chats| chats.into_iter().find(|chat| chat.id == id));
                drop(state);
                app::refresh_all(&s.borrow());
                if let Some(chat) = chat { app::open(&ui, &s, &chat); }
            }
            Ok(false) => {},
            Err(error) => app::append(&ui, "error", &format!("Cannot change project: {error}")),
        }
    });
    // Model and variant selection apply to the server session.
    let s = shared.clone();
    let weak = ui.as_weak();
    ui.on_select_model(move |model| {
        let ui = weak.unwrap();
        let id = ui.get_active_id() as i64;
        if s.borrow().ready.contains(&id) {
            app::send(&ui, &s.borrow(), json!({ "kind": "model", "id": id, "model": model.to_string() }));
        }
    });
    let s = shared.clone();
    let weak = ui.as_weak();
    ui.on_select_level(move |level| {
        let ui = weak.unwrap();
        let id = ui.get_active_id() as i64;
        if s.borrow().ready.contains(&id) && ui.get_thinking_levels().iter().any(|item| item == level) {
            app::send(&ui, &s.borrow(), json!({ "kind": "variant", "id": id, "variant": level.to_string() }));
        }
    });
    // Keep manual permission choices available if automatic approval fails.
    let s = shared.clone();
    let weak = ui.as_weak();
    ui.on_reply_permission(move |allow| {
        let ui = weak.unwrap();
        let id = ui.get_active_id() as i64;
        if let Some(request) = s.borrow().permissions.get(&id).cloned() {
            app::send(&ui, &s.borrow(), json!({ "kind": "permission", "id": id, "request": request, "decision": if allow { "once" } else { "reject" } }));
        }
    });
    // Persist keyboard zoom changes for future windows.
    let s = shared.clone();
    let weak = ui.as_weak();
    ui.on_zoom_changed(move |zoom| {
        if let Some(store) = &s.borrow().store {
            if let Err(error) = store.set_zoom(zoom) {
                if let Some(ui) = weak.upgrade() {
                    app::append(&ui, "error", &format!("Cannot save zoom: {error}"));
                }
            }
        }
    });
}
