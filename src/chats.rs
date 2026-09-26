//! Per-window chat actions sharing one backend.
use crate::{AppWindow, app, store::Chat};
use serde_json::{Value, json};
use slint::{ComponentHandle, Model};
use std::{cell::RefCell, rc::Rc};

// Open the first chat, creating one when the sidebar is empty.
pub fn open_first(ui: &AppWindow, shared: &Rc<RefCell<app::Shared>>) {
    let chats = app::show_chats(ui, &shared.borrow()).unwrap_or_default();
    if let Some(chat) = chats.first() {
        app::open(ui, shared, chat);
    } else if let Some(store) = &shared.borrow().store {
        if let Ok(chat) = store.create() {
            app::refresh_all(&shared.borrow());
            app::open(ui, shared, &chat);
        }
    }
}

// Wire sidebar, model, permission, and composer actions for one window.
pub fn wire(ui: &AppWindow, shared: &Rc<RefCell<app::Shared>>) {
    // Reuse a blank top chat instead of creating multiple empty sessions.
    let s = shared.clone();
    let weak = ui.as_weak();
    ui.on_new_chat(move || {
        let ui = weak.unwrap();
        let shared_ref = s.borrow();
        let Some(store) = &shared_ref.store else { return };
        match store.list() {
            Ok(chats) => {
                if let Some(chat) = chats.first().filter(|chat| !chat.started) {
                    let mut changed = false;
                    if let Ok(Some(project)) = store.last_project() {
                        if project != chat.project_id { changed = store.set_project(chat.id, project).unwrap_or(false); }
                    }
                    let updated = if changed { store.list().ok().and_then(|chats| chats.into_iter().find(|item| item.id == chat.id)) } else { None };
                    drop(shared_ref);
                    if let Some(updated) = updated {
                        app::refresh_all(&s.borrow());
                        app::open(&ui, &s, &updated);
                    } else { ui.invoke_open_session(chat.id as i32); }
                    return;
                }
            }
            Err(error) => {
                app::append(&ui, "error", &error.to_string());
                return;
            }
        }
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
    // Permission choices are explicit; never auto-approve agent actions.
    let s = shared.clone();
    let weak = ui.as_weak();
    ui.on_reply_permission(move |allow| {
        let ui = weak.unwrap();
        let id = ui.get_active_id() as i64;
        if let Some(request) = s.borrow().permissions.get(&id).cloned() {
            app::send(&ui, &s.borrow(), json!({ "kind": "permission", "id": id, "request": request, "decision": if allow { "once" } else { "reject" } }));
        }
    });
    // Send a prompt or answer the currently pending form.
    let s = shared.clone();
    let weak = ui.as_weak();
    ui.on_send_message(move |text| {
        let ui = weak.unwrap();
        let id = ui.get_active_id() as i64;
        let text = text.trim().to_owned();
        if text.is_empty() || !s.borrow().ready.contains(&id) {
            return;
        }
        let mut state = s.borrow_mut();
        if let Some(form) = state.forms.get(&id).cloned() {
            match serde_json::from_str::<Value>(&text) {
                Ok(Value::Object(answer)) => {
                    app::send(&ui, &state, json!({ "kind": "form", "id": id, "form": form, "answer": answer }));
                }
                _ => app::append(&ui, "error", "Answer the form with a JSON object, e.g. {\"name\":\"value\"}."),
            }
            return;
        }
        if state.busy.contains(&id) {
            return;
        }
        if !app::send(&ui, &state, json!({ "kind": "prompt", "id": id, "text": text })) {
            return;
        }
        app::append(&ui, "user", &text);
        ui.set_busy(true);
        ui.set_active_started(true);
        state.busy.insert(id);
        if let Some(store) = &state.store {
            store.mark_started(id).ok();
            if ui.get_active_title() == "New chat" {
                let title = text.chars().take(36).collect::<String>();
                if store.rename(id, &title).is_ok() {
                    ui.set_active_title(title.clone().into());
                    drop(state);
                    app::refresh_all(&s.borrow());
                    app::send(&ui, &s.borrow(), json!({ "kind": "rename", "id": id, "title": title }));
                }
            }
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
