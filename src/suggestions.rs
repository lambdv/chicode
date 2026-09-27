//! Filter slash and mention suggestions and distinguish UI actions from server commands.
use crate::{AppWindow, Suggestion, app};
use serde_json::{Value, json};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::{cell::RefCell, rc::Rc};

const LOCAL: &[(&str, &str)] = &[("new", "New chat"), ("agent", "Next agent"), ("model", "Choose model"),
    ("open", "Attach files"), ("workspace", "Toggle sidebar"), ("compact", "Compact conversation"),
    ("undo", "Revert last user turn"), ("redo", "Clear staged revert"), ("fork", "Fork conversation"), ("export", "Export conversation")];

/// Show a bounded suggestion list with stable actions and keyboard selection.
fn display(ui: &AppWindow, items: Vec<Suggestion>) {
    ui.set_suggestions(ModelRc::new(VecModel::from(items)));
    ui.set_suggestion_index(0);
}

/// Wire server catalogs and lightweight local filtering for one composer.
pub fn wire(ui: &AppWindow, shared: &Rc<RefCell<app::Shared>>) {
    let commands = Rc::new(RefCell::new(Vec::<Value>::new()));
    let s = commands.clone(); let state = shared.clone(); let weak = ui.as_weak();
    ui.on_draft_edited(move |text| {
        let ui = weak.unwrap(); let text = text.to_string();
        let text = text.get(..ui.get_draft_cursor().max(0) as usize).unwrap_or(&text);
        if text.starts_with('/') && !text.contains(char::is_whitespace) {
            let query = text[1..].to_lowercase();
            let mut items: Vec<_> = LOCAL.iter().filter(|(name, description)| {
                (name.contains(&query) || description.to_lowercase().contains(&query))
                    && (ui.get_active_started() || !["compact", "undo", "redo", "fork", "export"].contains(name))
                    && ((!ui.get_busy() && !ui.get_submitting()) || !["agent", "model"].contains(name))
            }).map(|(name, description)| Suggestion {
                label: format!("/{name}").into(), description: (*description).into(), kind: "local".into(), value: (*name).into(),
            }).collect();
            items.extend(s.borrow().iter().filter(|item| item["name"].as_str().unwrap_or("").to_lowercase().contains(&query)).map(|item| Suggestion {
                label: format!("/{}", item["name"].as_str().unwrap_or("")).into(), description: item["description"].as_str().unwrap_or("").into(),
                kind: "command".into(), value: item["name"].as_str().unwrap_or("").into(),
            }));
            display(&ui, items);
        } else if let Some(query) = text.split_whitespace().last().and_then(|word| word.strip_prefix('@')) {
            app::send(&ui, &state.borrow(), json!({"kind":"suggest", "id":ui.get_active_id(), "query":query}));
        } else { display(&ui, Vec::new()); }
    });
    let s = commands.clone(); let weak = ui.as_weak();
    ui.on_suggestion_event(move |kind, data| {
        let ui = weak.unwrap(); let value: Value = serde_json::from_str(&data).unwrap_or(Value::Null);
        if kind == "commands" { *s.borrow_mut() = value.as_array().cloned().unwrap_or_default(); return }
        let draft = ui.get_draft();
        let prefix = draft.get(..ui.get_draft_cursor().max(0) as usize).unwrap_or(&draft);
        let query = prefix.split_whitespace().last().and_then(|word| word.strip_prefix('@'));
        if query != value["query"].as_str() { return }
        display(&ui, value["items"].as_array().into_iter().flatten().map(|item| Suggestion {
            label: item["label"].as_str().unwrap_or("").into(), description: item["kind"].as_str().unwrap_or("").into(),
            kind: item["kind"].as_str().unwrap_or("").into(), value: item["value"].as_str().unwrap_or("").into(),
        }).collect());
    });
    let state = shared.clone(); let weak = ui.as_weak();
    ui.on_choose_suggestion(move |index| {
        let ui = weak.unwrap(); let Some(item) = ui.get_suggestions().row_data(index as usize) else { return };
        display(&ui, Vec::new());
        match item.kind.as_str() {
            "local" => {
                ui.set_draft("".into());
                match item.value.as_str() {
                    "new" => ui.invoke_new_chat(), "open" => ui.invoke_pick_attachments(),
                    "model" => ui.invoke_open_model_picker(), "workspace" => ui.invoke_toggle_sidebar(),
                    "agent" => {
                        let agents = ui.get_agents();
                        if agents.row_count() > 0 {
                            let index = agents.iter().position(|agent| agent.id == ui.get_current_agent()).map(|index| index + 1).unwrap_or(0) % agents.row_count();
                            ui.invoke_select_agent(agents.row_data(index).unwrap().id);
                        }
                    }
                    _ => { app::send(&ui, &state.borrow(), json!({"kind":"action", "id":ui.get_active_id(), "action":item.value.to_string()})); }
                }
            }
            "command" => { let text = format!("/{} ", item.value); ui.set_draft(text.clone().into()); ui.invoke_place_cursor(text.len() as i32); },
            _ => {
                ui.invoke_context_selected(json!({"kind":item.kind.to_string(), "name":item.label.to_string(), "uri":item.value.to_string()}).to_string().into());
                let draft = ui.get_draft().to_string();
                let cursor = ui.get_draft_cursor().max(0) as usize;
                if let Some(start) = draft.get(..cursor).and_then(|prefix| prefix.rfind('@')) {
                    let insertion = format!("@{} ", item.label);
                    ui.set_draft(format!("{}{}{}", &draft[..start], insertion, &draft[cursor..]).into());
                    ui.invoke_place_cursor((start + insertion.len()) as i32);
                }
            }
        }
    });
}
