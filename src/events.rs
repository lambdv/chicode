//! Route OpenCode bridge events to the selected Slint conversation.
use crate::{AgentOption, AppWindow, ChatLine, app};
use serde_json::{Value, json};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::{cell::RefCell, collections::HashMap, rc::Rc};

// Decode a conversation row without discarding message metadata.
fn chat_line(value: &Value) -> Option<ChatLine> {
    Some(ChatLine {
        id: value["id"].as_str().unwrap_or("").into(),
        role: value["role"].as_str()?.into(),
        body: value["body"].as_str().unwrap_or("").into(),
        message: value["message"].as_str().unwrap_or("").into(),
        part: value["part"].as_str().unwrap_or("").into(),
        status: value["status"].as_str().unwrap_or("").into(),
        elapsed: value["elapsed"].as_str().unwrap_or("").into(),
        expanded: value["expanded"].as_bool().unwrap_or(false),
        details: value["details"].as_str().unwrap_or("").into(),
    })
}

// Connect model search and bridge notifications to the window.
pub fn connect(ui: &AppWindow, state: &Rc<RefCell<app::Shared>>) {
    let weak = ui.as_weak();
    ui.on_search_models(move |query| {
        let ui = weak.unwrap();
        let query = query.trim().to_lowercase();
        ui.set_filtered_models(ModelRc::new(VecModel::from(
            ui.get_all_models().iter().filter(|model| model.to_lowercase().contains(&query)).collect::<Vec<_>>(),
        )));
    });
    let shared = state.clone();
    let weak = ui.as_weak();
    let mut positions = HashMap::<String, (usize, usize)>::new();
    let mut indexed_session = -1;
    ui.on_opencode_event(move |id, kind, data| {
        let ui = weak.unwrap();
        let id = id as i64;
        let value = serde_json::from_str::<Value>(&data).unwrap_or(Value::Null);
        ui.invoke_composer_event(id as i32, kind.clone(), data.clone());
        // Keep background chat state up to date even when another chat is visible.
        match kind.as_str() {
            "ready" => { shared.borrow_mut().ready.insert(id); }
            "session" => {
                if let Some(session) = value.as_str() {
                    if let Some(store) = &shared.borrow().store {
                        if let Err(error) = store.set_session(id, session) {
                            app::append(&ui, "error", &format!("Cannot save session: {error}"));
                        }
                    }
                    shared.borrow_mut().ready.insert(id);
                }
            }
            "busy" => {
                if value == true {
                    shared.borrow_mut().busy.insert(id);
                } else {
                    shared.borrow_mut().busy.remove(&id);
                }
            }
            "submitted" if value["ok"] == true => {
                let state = shared.borrow();
                if let Some(store) = &state.store {
                    store.mark_started(id).ok();
                    if let Some(chat) = store.list().ok().and_then(|items| items.into_iter().find(|chat| chat.id == id)) {
                        if chat.title == "New chat" {
                            let title: String = value["text"].as_str().unwrap_or("Attachments").chars().take(36).collect();
                            let title = if title.is_empty() { "Attachments".into() } else { title };
                            if store.rename(id, &title).is_ok() {
                                app::send(&ui, &state, json!({"kind":"rename", "id":id, "title":title}));
                                if ui.get_active_id() == id as i32 { ui.set_active_title(title.into()); }
                            }
                        }
                    }
                }
                app::refresh_all(&state);
            }
            "permission" => {
                if let Some(request) = value["id"].as_str() {
                    shared.borrow_mut().permissions.insert(id, request.into());
                } else {
                    shared.borrow_mut().permissions.remove(&id);
                }
            }
            "exited" => {
                shared.borrow_mut().ready.clear();
                shared.borrow_mut().busy.clear();
            }
            _ => {}
        }
        if id != -1 && ui.get_active_id() != id as i32 {
            return;
        }
        if indexed_session != ui.get_active_id() { positions.clear(); indexed_session = ui.get_active_id(); }
        match kind.as_str() {
            "history" => {
                if let Some(messages) = value.as_array() {
                    ui.invoke_before_update();
                    let old = ui.get_messages();
                    let previous: HashMap<_, _> = old.iter().filter(|row| row.expanded).map(|row| (row.id.clone(), row)).collect();
                    let mut rows = messages.iter().filter_map(chat_line).map(|mut row| {
                        if let Some(previous) = previous.get(&row.id) {
                            row.expanded = previous.expanded;
                            row.details = previous.details.clone();
                        }
                        row
                    }).collect::<Vec<_>>();
                    rows.extend(old.iter().filter(|row| row.id.starts_with("local/")));
                    positions.clear();
                    for (index, row) in rows.iter().enumerate() {
                        positions.entry(row.message.to_string()).and_modify(|(_, count)| *count += 1).or_insert((index, 1));
                    }
                    ui.set_messages(ModelRc::new(VecModel::from(rows)));
                    ui.invoke_scroll_to_bottom();
                }
            }
            "parts" => {
                let Some(rows) = value["rows"].as_array() else { return };
                ui.invoke_before_update();
                let messages = ui.get_messages();
                let model = messages.as_any().downcast_ref::<VecModel<ChatLine>>().unwrap();
                let message = value["message"].as_str().unwrap_or("");
                let (start, count) = positions.get(message).copied().unwrap_or((model.row_count(), 0));
                let old: Vec<_> = (start..start + count).filter_map(|index| model.row_data(index)).collect();
                let next: Vec<_> = rows.iter().filter_map(chat_line).map(|mut row| {
                    if let Some(previous) = old.iter().find(|item| item.id == row.id) {
                        row.expanded = previous.expanded;
                        row.details = previous.details.clone();
                    }
                    row
                }).collect();
                let next_count = next.len();
                // Most deltas only change one existing row.
                if old.len() == next.len() && old.iter().zip(&next).all(|(a, b)| a.id == b.id) {
                    for (index, row) in next.into_iter().enumerate() { if old[index] != row { model.set_row_data(start + index, row); } }
                } else {
                    for _ in &old { model.remove(start); }
                    for (index, row) in next.into_iter().enumerate() { model.insert(start + index, row); }
                }
                if count != next_count {
                    for (offset, _) in positions.values_mut() { if *offset > start { *offset = (*offset as isize + next_count as isize - count as isize) as usize; } }
                }
                positions.insert(message.into(), (start, next_count));
                ui.invoke_scroll_to_bottom();
            }
            "details" => {
                let messages = ui.get_messages();
                let model = messages.as_any().downcast_ref::<VecModel<ChatLine>>().unwrap();
                if let Some(index) = model.iter().position(|row| row.id == value["row"].as_str().unwrap_or("")) {
                    let mut row = model.row_data(index).unwrap();
                    row.details = value["body"].as_str().unwrap_or("").into();
                    model.set_row_data(index, row);
                }
            }
            "submitted" => {
                ui.set_submitting(false);
                if value["ok"] == true {
                    ui.set_active_started(true);
                    if ui.get_draft() == value["text"].as_str().unwrap_or("") { ui.set_draft("".into()); }
                }
            }
            "restore-draft" => {
                if ui.get_draft().is_empty() { ui.set_draft(value.as_str().unwrap_or("").into()); }
                else { app::append(&ui, "status", "Undo applied; existing draft preserved."); }
            }
            "forked" => {
                if let Some(session) = value["id"].as_str() {
                    let state = shared.borrow();
                    if let Some(store) = &state.store {
                        if let Ok(mut chat) = store.create() {
                            let result = store.set_project(chat.id, ui.get_active_project() as i64)
                                .and_then(|_| store.set_session(chat.id, session));
                            if let Err(error) = result { app::append(&ui, "error", &error.to_string()); return; }
                            store.mark_started(chat.id).ok();
                            chat.session = session.into(); chat.started = true; chat.project_id = ui.get_active_project() as i64;
                            drop(state); app::refresh_all(&shared.borrow()); app::open(&ui, &shared, &chat);
                        }
                    }
                }
            }
            "export" => {
                let text = serde_json::to_string_pretty(&value).unwrap_or_default(); let weak = ui.as_weak();
                std::thread::spawn(move || {
                    if let Some(path) = rfd::FileDialog::new().set_file_name("conversation.json").save_file() {
                        if let Err(error) = std::fs::write(path, text) { let _ = weak.upgrade_in_event_loop(move |ui| app::append(&ui, "error", &error.to_string())); }
                    }
                });
            }
            "agent" => {
                let id = value.as_str().unwrap_or("");
                ui.set_current_agent(id.into());
                ui.set_current_agent_name(ui.get_agents().iter().find(|agent| agent.id == id)
                    .map(|agent| agent.name).unwrap_or_default());
            }
            "question" => ui.invoke_question_event(data.clone()),
            "question-busy" => ui.set_question_busy(value == true),
            "commands" | "suggestions" => ui.invoke_suggestion_event(kind.clone(), data.clone()),
            "attachment-view" => ui.invoke_preview_attachment(data.clone()),
            "agents" => {
                if let Some(items) = value.as_array() {
                    ui.set_agents(ModelRc::new(VecModel::from(items.iter().filter_map(|item| Some(AgentOption {
                        id: item["id"].as_str()?.into(), name: item["name"].as_str()?.into(),
                    })).collect::<Vec<_>>() )));
                    ui.set_current_agent_name(ui.get_agents().iter().find(|agent| agent.id == ui.get_current_agent())
                        .map(|agent| agent.name).unwrap_or_default());
                }
            }
            "capabilities" => {
                if let Ok(items) = serde_json::from_value::<Vec<String>>(value) { ui.set_input_types(ModelRc::new(VecModel::from(items.into_iter().map(Into::into).collect::<Vec<_>>()))); }
            }
            "inbox" => ui.set_pending_inputs(value.as_array().map(|items| items.iter().map(|item| format!("Pending · {}", item["text"].as_str().unwrap_or(""))).collect::<Vec<_>>().join("\n")).unwrap_or_default().into()),
            "busy" => ui.set_busy(value == true),
            "model" => ui.set_current_model(value.as_str().unwrap_or("").into()),
            "variant" => ui.set_current_level(value.as_str().unwrap_or("").into()),
            "variants" => {
                if let Ok(levels) = serde_json::from_value::<Vec<String>>(value) {
                    ui.set_thinking_levels(ModelRc::new(VecModel::from(levels.into_iter().map(Into::into).collect::<Vec<_>>())));
                }
            }
            "models" => {
                if let Ok(models) = serde_json::from_value::<Vec<String>>(value) {
                    // Cache for instant second windows.
                    shared.borrow_mut().models = models.clone();
                    ui.set_all_models(ModelRc::new(VecModel::from(models.into_iter().map(Into::into).collect::<Vec<_>>())));
                    ui.invoke_search_models("".into());
                }
            }
            "permission" => ui.set_permission_text(value["text"].as_str().unwrap_or("").into()),
            "error" | "exited" => {
                if kind == "exited" {
                    ui.set_busy(false);
                }
                app::append(&ui, "error", value.as_str().unwrap_or("OpenCode connection failed"));
            }
            _ => {}
        }
    });
}
