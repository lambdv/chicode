//! Maintain partial V2 form answers and route question reply/cancel operations.
use crate::{AppWindow, Choice, app};
use serde_json::{Map, Value, json};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::{cell::RefCell, collections::HashMap, rc::Rc};

#[derive(Default, Clone)]
struct Questions { request: Value, answers: Map<String, Value>, index: usize }

/// Return visible fields whose conditional predicates match the current answers.
fn fields(state: &Questions) -> Vec<&Value> {
    state.request["fields"].as_array().into_iter().flatten().filter(|field| {
        field["hidden"] != true && field["when"].as_array().is_none_or(|conditions| conditions.iter().all(|condition| {
            let equal = state.answers.get(condition["key"].as_str().unwrap_or("")) == Some(&condition["value"]);
            if condition["op"] == "neq" { !equal } else { equal }
        }))
    }).collect()
}

/// Present the current field without losing answers to other fields.
fn show(ui: &AppWindow, state: &Questions) {
    let fields = fields(state);
    let Some(field) = fields.get(state.index.min(fields.len().saturating_sub(1))) else { ui.set_question_title("".into()); return };
    ui.set_question_title(state.request["title"].as_str().unwrap_or("Question").into());
    ui.set_question_text(format!("{}\n{}", field["title"].as_str().unwrap_or(field["key"].as_str().unwrap_or("")), field["description"].as_str().unwrap_or("")).into());
    ui.set_question_progress(format!("{} / {}", state.index + 1, fields.len()).into());
    ui.set_question_first(state.index == 0);
    ui.set_question_last(state.index + 1 >= fields.len());
    ui.set_question_custom(field["custom"] == true || field["options"].as_array().is_none());
    let answer = state.answers.get(field["key"].as_str().unwrap_or(""));
    let options = field["options"].as_array().cloned().unwrap_or_default();
    ui.set_question_choices(ModelRc::new(VecModel::from(options.iter().map(|option| Choice {
        label: option["label"].as_str().unwrap_or("").into(),
        description: option["description"].as_str().unwrap_or("").into(),
        selected: answer.is_some_and(|answer| answer == &option["value"] || answer.as_array().is_some_and(|items| items.contains(&option["value"]))),
    }).collect::<Vec<_>>())));
    let custom = match answer {
        Some(Value::String(text)) if !options.iter().any(|option| option["value"] == *text) => text.clone(),
        Some(value) if value.is_number() || value.is_boolean() => value.to_string(),
        Some(Value::Array(values)) => values.iter().find(|value| !options.iter().any(|option| &option["value"] == *value)).and_then(Value::as_str).unwrap_or("").to_owned(),
        _ => String::new(),
    };
    ui.set_question_answer(custom.into());
}

/// Save typed answers with the server field's type before navigation or submission.
fn save(ui: &AppWindow, state: &mut Questions) -> bool {
    let list = fields(state);
    let Some(field) = list.get(state.index) else { return true };
    let key = field["key"].as_str().unwrap_or("").to_owned();
    let text = ui.get_question_answer().to_string();
    if text.is_empty() {
        let options = field["options"].as_array().cloned().unwrap_or_default();
        let previous = state.answers.get(&key).cloned();
        match previous {
            Some(Value::Array(mut values)) => { values.retain(|value| options.iter().any(|option| &option["value"] == value)); state.answers.insert(key, Value::Array(values)); },
            Some(value) if !options.iter().any(|option| option["value"] == value) => { state.answers.remove(&key); },
            _ => {},
        }
        return true
    }
    let value = match field["type"].as_str().unwrap_or("string") {
        "number" | "integer" | "boolean" => match serde_json::from_str::<Value>(&text) {
            Ok(value) if match field["type"].as_str() { Some("boolean") => value.is_boolean(), Some("integer") => value.is_i64() || value.is_u64(), _ => value.is_number() } => value,
            _ => { app::append(ui, "error", "Enter a valid number or boolean for this field."); return false; }
        },
        "multiselect" => {
            let mut values = state.answers.get(&key).and_then(Value::as_array).cloned().unwrap_or_default();
            values.retain(|value| field["options"].as_array().is_some_and(|options| options.iter().any(|option| &option["value"] == value)));
            if !values.contains(&json!(text)) { values.push(json!(text)); }
            Value::Array(values)
        },
        _ => json!(text),
    };
    state.answers.insert(key, value);
    true
}

/// Wire one window's question dock; request IDs guard partial-answer restoration.
pub fn wire(ui: &AppWindow, shared: &Rc<RefCell<app::Shared>>) {
    let state = Rc::new(RefCell::new(Questions::default()));
    let mut saved = HashMap::<String, Questions>::new();
    let s = state.clone(); let weak = ui.as_weak();
    ui.on_question_event(move |data| {
        let ui = weak.unwrap();
        let request: Value = serde_json::from_str(&data).unwrap_or(Value::Null);
        let mut state = s.borrow_mut();
        if state.request["id"] != request["id"] {
            if let Some(id) = state.request["id"].as_str().map(str::to_owned) {
                save(&ui, &mut state);
                saved.insert(id, state.clone());
            }
            if let Some(previous) = saved.remove(request["id"].as_str().unwrap_or("")) { *state = previous; }
            else {
                *state = Questions { request: request.clone(), ..Default::default() };
                for field in request["fields"].as_array().into_iter().flatten() {
                    if let (Some(key), Some(value)) = (field["key"].as_str(), field.get("default")) { state.answers.insert(key.into(), value.clone()); }
                }
            }
            ui.set_question_minimized(false);
            ui.set_question_busy(false);
        }
        state.request = request;
        show(&ui, &state);
    });
    let s = state.clone(); let weak = ui.as_weak();
    ui.on_question_choose(move |index| {
        let ui = weak.unwrap(); let mut state = s.borrow_mut();
        if ui.get_question_busy() { return }
        if !save(&ui, &mut state) { return }
        let list = fields(&state);
        let Some(field) = list.get(state.index) else { return };
        let Some(option) = field["options"].as_array().and_then(|items| items.get(index as usize)) else { return };
        let key = field["key"].as_str().unwrap_or("").to_owned();
        let value = option["value"].clone();
        let answer = if field["type"] == "multiselect" {
            let mut values = state.answers.get(&key).and_then(Value::as_array).cloned().unwrap_or_default();
            if values.contains(&value) { values.retain(|item| item != &value); } else { values.push(value); }
            Value::Array(values)
        } else { value };
        state.answers.insert(key, answer);
        state.index = state.index.min(fields(&state).len().saturating_sub(1));
        show(&ui, &state);
    });
    let s = state.clone(); let weak = ui.as_weak();
    ui.on_question_navigate(move |direction| {
        let ui = weak.unwrap(); let mut state = s.borrow_mut();
        if save(&ui, &mut state) { state.index = (state.index as i32 + direction).clamp(0, fields(&state).len().saturating_sub(1) as i32) as usize; show(&ui, &state); }
    });
    let s = state.clone(); let shared = shared.clone(); let weak = ui.as_weak();
    ui.on_question_finish(move |dismiss| {
        let ui = weak.unwrap(); let mut state = s.borrow_mut();
        if ui.get_question_busy() || state.request.is_null() { return }
        if !dismiss && !save(&ui, &mut state) { return }
        if app::send(&ui, &shared.borrow(), json!({ "kind": if dismiss { "form-cancel" } else { "form" }, "id": ui.get_active_id(), "form": state.request["id"], "answer": state.answers })) { ui.set_question_busy(true); }
    });
}
