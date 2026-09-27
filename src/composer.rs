//! Composer state, stable admission IDs, tool expansion, and selection controls.
use crate::{AppWindow, ChatLine, app};
use serde_json::{Value, json};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::{cell::RefCell, collections::HashMap, rc::Rc};

#[derive(Default)]
struct Draft { text: String, files: Vec<Value>, agents: Vec<Value>, history: Vec<String>, index: usize, saved: String, submitting: bool, loading: i32, retry: Option<(String, Value)> }

/// Refresh attachment cards without replacing the editor or its cursor.
fn cards(ui: &AppWindow, draft: &Draft) {
    ui.set_attachments(ModelRc::new(VecModel::from(draft.files.iter().map(|file| file["name"].as_str().unwrap_or("Attachment").into())
        .chain(draft.agents.iter().map(|agent| format!("@{}", agent["name"].as_str().unwrap_or("")).into())).collect::<Vec<slint::SharedString>>())));
}

/// Wire native composer behavior for one window and retain drafts across session switches.
pub fn wire(ui: &AppWindow, shared: &Rc<RefCell<app::Shared>>) {
    ui.on_render_markdown(|text| crate::markdown::render(&text));
    ui.on_text_length(|text| text.len() as i32);
    let drafts = Rc::new(RefCell::new(HashMap::<i32, Draft>::new()));
    let active = Rc::new(RefCell::new(-1));
    let s = drafts.clone(); let current = active.clone(); let weak = ui.as_weak();
    ui.on_switch_draft(move |id| {
        let ui = weak.unwrap(); let mut drafts = s.borrow_mut();
        drafts.entry(*current.borrow()).or_default().text = ui.get_draft().into();
        let draft = drafts.entry(id).or_default();
        ui.set_draft(draft.text.clone().into()); ui.set_submitting(draft.submitting); cards(&ui, draft); *current.borrow_mut() = id;
        ui.set_loading_attachments(draft.loading > 0);
        ui.set_suggestions(ModelRc::default());
    });
    let s = drafts.clone(); let weak = ui.as_weak();
    ui.on_attachment_loading(move |id, delta| {
        let ui = weak.unwrap(); let mut drafts = s.borrow_mut(); let draft = drafts.entry(id).or_default();
        draft.loading = (draft.loading + delta).max(0);
        if ui.get_active_id() == id { ui.set_loading_attachments(draft.loading > 0); }
    });
    let s = drafts.clone(); let weak = ui.as_weak();
    ui.on_attachment_ready(move |id, data| {
        let ui = weak.unwrap(); let Ok(file) = serde_json::from_str::<Value>(&data) else { return };
        let mut drafts = s.borrow_mut(); let draft = drafts.entry(id).or_default();
        draft.files.push(file); if ui.get_active_id() == id { cards(&ui, draft); }
    });
    let s = drafts.clone(); let weak = ui.as_weak();
    ui.on_remove_attachment(move |index| {
        let ui = weak.unwrap(); let mut drafts = s.borrow_mut(); let draft = drafts.entry(ui.get_active_id()).or_default();
        if index < 0 { return }
        let index = index as usize;
        if index < draft.files.len() { draft.files.remove(index); }
        else if index - draft.files.len() < draft.agents.len() { draft.agents.remove(index - draft.files.len()); }
        cards(&ui, draft);
    });
    let s = drafts.clone(); let weak = ui.as_weak();
    ui.on_context_selected(move |data| {
        let ui = weak.unwrap(); let Ok(item) = serde_json::from_str::<Value>(&data) else { return };
        let mut drafts = s.borrow_mut(); let draft = drafts.entry(ui.get_active_id()).or_default();
        if item["kind"] == "agent" { draft.agents.push(json!({"name":item["name"]})); }
        else { draft.files.push(item); }
        cards(&ui, draft);
    });
    let s = drafts.clone(); let weak = ui.as_weak();
    ui.on_prompt_history(move |direction| {
        let ui = weak.unwrap(); let mut drafts = s.borrow_mut(); let draft = drafts.entry(ui.get_active_id()).or_default();
        if draft.index == draft.history.len() { draft.saved = ui.get_draft().into(); }
        draft.index = (draft.index as i32 + direction).clamp(0, draft.history.len() as i32) as usize;
        draft.history.get(draft.index).unwrap_or(&draft.saved).clone().into()
    });
    let s = drafts.clone(); let shared_send = shared.clone(); let weak = ui.as_weak();
    ui.on_send_message(move |text| {
        let ui = weak.unwrap(); let id = ui.get_active_id(); let text = text.to_string();
        let mut drafts = s.borrow_mut(); let draft = drafts.entry(id).or_default();
        if (text.trim().is_empty() && draft.files.is_empty()) || ui.get_submitting() || draft.loading > 0 || !shared_send.borrow().ready.contains(&(id as i64)) { return }
        // Capability checks use the current selected model, never its display name.
        for file in &draft.files {
            let mime = file["mime"].as_str().unwrap_or("text/plain");
            let kind = if mime.starts_with("image/") { "image" } else if mime == "application/pdf" { "pdf" } else { "text" };
            if !ui.get_input_types().iter().any(|value| value == kind || value == mime) { app::append(&ui, "error", &format!("Selected model does not support {kind} attachments.")); return }
        }
        let payload = json!({ "kind": "prompt", "id": id, "text": text, "agents":draft.agents, "files": draft.files.iter().map(|file| json!({"uri": file["uri"], "name": file["name"]})).collect::<Vec<_>>() });
        let message_id = draft.retry.as_ref().filter(|(_, previous)| previous == &payload).map(|(id, _)| id.clone()).unwrap_or_else(|| format!("msg_{}", uuid::Uuid::new_v4().simple()));
        draft.retry = Some((message_id.clone(), payload.clone()));
        let mut request = payload; request["messageID"] = json!(message_id);
        if app::send(&ui, &shared_send.borrow(), request) {
            ui.set_submitting(true); draft.submitting = true;
        }
    });
    let s = drafts.clone(); let weak = ui.as_weak();
    ui.on_composer_event(move |id, kind, data| {
        let ui = weak.unwrap(); let value: Value = serde_json::from_str(&data).unwrap_or(Value::Null);
        if kind == "submitted" { s.borrow_mut().entry(id).or_default().submitting = false; }
        if kind == "history" {
            let mut drafts = s.borrow_mut(); let draft = drafts.entry(id).or_default();
            draft.history = value.as_array().into_iter().flatten().filter(|row| row["role"] == "user").filter_map(|row| row["body"].as_str().map(str::to_owned)).collect();
            draft.index = draft.history.len();
        }
        if kind == "submitted" && value["ok"] == true {
            let mut drafts = s.borrow_mut(); let draft = drafts.entry(id).or_default();
            let text = value["text"].as_str().unwrap_or("");
            draft.history.push(text.into()); draft.index = draft.history.len();
            // Remove only submitted attachments; newly added ones belong to the next draft.
            if let Some((_, payload)) = draft.retry.take() {
                let submitted = payload["files"].as_array().cloned().unwrap_or_default();
                draft.files.retain(|file| !submitted.iter().any(|sent| sent["uri"] == file["uri"]));
            }
            draft.agents.clear(); if draft.text == text { draft.text.clear(); }
            if ui.get_active_id() == id { cards(&ui, draft); }
        }
    });
    let s = shared.clone(); let weak = ui.as_weak();
    ui.on_open_attachment(move |message, part| {
        let ui = weak.unwrap();
        app::send(&ui, &s.borrow(), json!({"kind":"attachment", "id":ui.get_active_id(), "message":message.to_string(), "part":part.to_string()}));
    });
    let s = shared.clone(); let weak = ui.as_weak();
    ui.on_interrupt(move || { let ui = weak.unwrap(); app::send(&ui, &s.borrow(), json!({"kind":"interrupt", "id":ui.get_active_id()})); });
    let s = shared.clone(); let weak = ui.as_weak();
    ui.on_select_agent(move |agent| { let ui = weak.unwrap(); app::send(&ui, &s.borrow(), json!({"kind":"agent", "id":ui.get_active_id(), "agent":agent.to_string()})); });
    let s = shared.clone(); let weak = ui.as_weak();
    ui.on_toggle_tool(move |id| {
        let ui = weak.unwrap(); let rows = ui.get_messages(); let model = rows.as_any().downcast_ref::<VecModel<ChatLine>>().unwrap();
        if let Some(index) = model.iter().position(|row| row.id == id) {
            let mut row = model.row_data(index).unwrap(); row.expanded = !row.expanded;
            app::send(&ui, &s.borrow(), json!({"kind":"details", "id":ui.get_active_id(), "row":id.to_string(), "message":row.message.to_string(), "part":row.part.to_string(), "open":row.expanded}));
            model.set_row_data(index, row);
        }
    });
    let weak = ui.as_weak();
    ui.on_copy_response(move |id| {
        let ui = weak.unwrap();
        let text = ui.get_messages().iter().filter(|row| row.message == id && row.role == "assistant").map(|row| row.body.to_string()).collect::<Vec<_>>().join("\n\n");
        if let Err(error) = arboard::Clipboard::new().and_then(|mut clipboard| clipboard.set_text(text)) { app::append(&ui, "error", &format!("Cannot copy response: {error}")); }
    });
}
