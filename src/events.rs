//! Route OpenCode bridge events to the selected Slint conversation.
use crate::{AppWindow, ChatLine, app};
use serde_json::Value;
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::{cell::RefCell, rc::Rc};

// Decode a conversation row without discarding message metadata.
fn chat_line(value: &Value) -> Option<ChatLine> {
    Some(ChatLine {
        id: value["id"].as_str().unwrap_or("").into(),
        role: value["role"].as_str()?.into(),
        body: value["body"].as_str().unwrap_or("").into(),
        model: value["model"].as_str().unwrap_or("").into(),
        tools: value["tools"].as_i64().unwrap_or(0) as i32,
        thinking: value["thinking"].as_bool().unwrap_or(false),
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
    ui.on_opencode_event(move |id, kind, data| {
        let ui = weak.unwrap();
        let id = id as i64;
        let value = serde_json::from_str::<Value>(&data).unwrap_or(Value::Null);
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
            "permission" => {
                if let Some(request) = value["id"].as_str() {
                    shared.borrow_mut().permissions.insert(id, request.into());
                } else {
                    shared.borrow_mut().permissions.remove(&id);
                }
            }
            "form" => {
                if let Some(form) = value["id"].as_str() {
                    shared.borrow_mut().forms.insert(id, form.into());
                } else {
                    shared.borrow_mut().forms.remove(&id);
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
        match kind.as_str() {
            "history" => {
                if let Some(messages) = value.as_array() {
                    ui.set_messages(ModelRc::new(VecModel::from(messages.iter().filter_map(chat_line).collect::<Vec<_>>())));
                    ui.invoke_scroll_to_bottom();
                }
            }
            "live" => {
                let Some(line) = chat_line(&value) else { return };
                let messages = ui.get_messages();
                let model = messages.as_any().downcast_ref::<VecModel<ChatLine>>().unwrap();
                if let Some(index) = (0..model.row_count()).find(|&i| model.row_data(i).is_some_and(|row| row.id == line.id)) {
                    model.set_row_data(index, line);
                } else {
                    model.push(line);
                }
                ui.invoke_scroll_to_bottom();
            }
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
            "form" => ui.set_form_text(value["text"].as_str().unwrap_or("").into()),
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
