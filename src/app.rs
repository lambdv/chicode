//! Shared runtime and fast in-process window creation.
use crate::{AppWindow, ChatLine, ChatRow, ProjectRow, chats, events, opencode, store::{Chat, Project, Store}};
use serde_json::{Value, json};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::{cell::RefCell, collections::{HashMap, HashSet}, error::Error, rc::Rc, sync::{Arc, Mutex, mpsc::Sender}};

// State shared by every window in this process.
pub struct Shared {
    pub store: Option<Store>,
    pub bridge: Option<Sender<Value>>,
    pub ready: HashSet<i64>,
    pub busy: HashSet<i64>,
    pub permissions: HashMap<i64, String>,
    pub forms: HashMap<i64, String>,
    pub models: Vec<String>,
    pub directory: String,
    pub windows: Arc<Mutex<Vec<slint::Weak<AppWindow>>>>,
}

/// Assign a compact, stable monogram and muted project tint.
fn badge(project: &Project) -> (slint::SharedString, slint::Color) {
    let initials = project.name.split(|c: char| !c.is_alphanumeric())
        .filter_map(|part| part.chars().next()).take(2).collect::<String>();
    let initials = if initials.chars().count() < 2 {
        project.name.chars().take(2).collect::<String>()
    } else { initials };
    let palette = [(91, 105, 124), (111, 91, 112), (92, 113, 103), (126, 102, 91),
        (94, 105, 132), (121, 104, 118), (112, 113, 92), (99, 116, 120)];
    let (r, g, b) = palette[project.color as usize % palette.len()];
    (initials.to_uppercase().into(), slint::Color::from_rgb_u8(r, g, b))
}

/// Build small UI records from stored projects and sessions.
fn rows(chats: &[Chat], projects: &[Project]) -> Vec<ChatRow> {
    chats.iter().map(|chat| {
        let project = projects.iter().find(|project| project.id == chat.project_id);
        let (initials, tint) = project.map(badge).unwrap_or_default();
        ChatRow { id: chat.id as i32, title: chat.title.clone().into(), initials, tint }
    }).collect()
}

/// Repopulate the project dropdown in every window.
pub fn refresh_projects(shared: &Shared) {
    let Some(store) = &shared.store else { return };
    let Ok(projects) = store.projects() else { return };
    let model = ModelRc::new(VecModel::from(projects.iter().map(|project| {
        let (initials, tint) = badge(project);
        ProjectRow { id: project.id as i32, name: project.name.clone().into(), initials, tint }
    }).collect::<Vec<_>>()));
    for weak in shared.windows.lock().unwrap().iter() {
        if let Some(ui) = weak.upgrade() { ui.set_projects(model.clone()); }
    }
}

/// Register a selected folder and apply it to any currently empty chat.
pub fn add_project(folder: std::path::PathBuf, target_id: i32) {
    APP.with(|slot| {
        let Some(shared) = slot.borrow().clone() else { return };
        let state = shared.borrow();
        let Some(store) = &state.store else { return };
        let project = match store.add_project(&folder).and_then(|project| {
            store.select_project(project.id)?;
            Ok(project)
        }) {
            Ok(project) => project,
            Err(error) => {
                for weak in state.windows.lock().unwrap().iter() {
                    if let Some(ui) = weak.upgrade() { append(&ui, "error", &format!("Cannot add project: {error}")); }
                }
                return;
            }
        };
        drop(state);
        refresh_projects(&shared.borrow());
        // Apply the choice to the chat that opened the dialog, if it is still empty.
        let windows: Vec<_> = shared.borrow().windows.lock().unwrap().iter().filter_map(|weak| weak.upgrade()).collect();
        for ui in windows {
            if !ui.get_active_started() && target_id >= 0 && ui.get_active_id() == target_id {
                ui.invoke_select_project(project.id as i32);
            }
        }
    });
}

// UI-thread homes for shared state and strong windows.
thread_local! {
    static APP: RefCell<Option<Rc<RefCell<Shared>>>> = RefCell::new(None);
    static ALIVE: RefCell<Vec<AppWindow>> = RefCell::new(Vec::new());
}

/// Create the shared runtime before the first paint.
pub fn init_shared(directory: String) -> Rc<RefCell<Shared>> {
    let shared = Rc::new(RefCell::new(Shared {
        store: None, bridge: None, ready: HashSet::new(), busy: HashSet::new(),
        permissions: HashMap::new(), forms: HashMap::new(), models: Vec::new(),
        directory, windows: Arc::new(Mutex::new(Vec::new())),
    }));
    APP.with(|slot| *slot.borrow_mut() = Some(shared.clone()));
    shared
}

// Keep a window alive; drop hidden ones so memory stays flat.
fn keep(ui: AppWindow) -> slint::Weak<AppWindow> {
    let weak = ui.as_weak();
    ALIVE.with(|slot| {
        let mut alive = slot.borrow_mut();
        alive.retain(|w| w.window().is_visible());
        alive.push(ui);
    });
    weak
}

/// Open windows requested by second launches.
pub fn drain_requests() {
    let dirs = crate::instance::take_queued();
    if dirs.is_empty() {
        return;
    }
    APP.with(|slot| {
        let Some(shared) = slot.borrow().clone() else {
            for dir in dirs {
                crate::instance::queue(dir);
            }
            return;
        };
        for dir in dirs {
            let dir = if dir.is_empty() { shared.borrow().directory.clone() } else { dir };
            if let Ok(weak) = create_window(&shared, dir) {
                if shared.borrow().store.is_some() {
                    if let Some(next) = weak.upgrade() {
                        chats::open_first(&next, &shared);
                    }
                }
            }
        }
    });
}

/// Add one chat list to a window.
pub fn show_chats(ui: &AppWindow, shared: &Shared) -> Result<Vec<Chat>, Box<dyn Error>> {
    let Some(store) = &shared.store else { return Ok(Vec::new()) };
    let chats = store.list()?;
    ui.set_sessions(ModelRc::new(VecModel::from(rows(&chats, &store.projects()?))));
    Ok(chats)
}

/// Refresh every open sidebar after a local change.
pub fn refresh_all(shared: &Shared) {
    let Some(store) = &shared.store else { return };
    if let (Ok(chats), Ok(projects)) = (store.list(), store.projects()) {
        let model = ModelRc::new(VecModel::from(rows(&chats, &projects)));
        for weak in shared.windows.lock().unwrap().iter() {
            if let Some(ui) = weak.upgrade() {
                ui.set_sessions(model.clone());
            }
        }
    }
}

/// Add a message to the current conversation.
pub fn append(ui: &AppWindow, role: &str, body: &str) {
    let messages = ui.get_messages();
    let model = messages.as_any().downcast_ref::<VecModel<ChatLine>>().unwrap();
    model.push(ChatLine { id: "".into(), role: role.into(), body: body.into(), model: "".into(), tools: 0, thinking: false });
    ui.invoke_scroll_to_bottom();
}

/// Queue one command, or report that the backend is still starting.
pub fn send(ui: &AppWindow, shared: &Shared, value: Value) -> bool {
    let Some(bridge) = &shared.bridge else {
        append(ui, "error", "Starting OpenCode connection...");
        return false;
    };
    if bridge.send(value).is_err() {
        append(ui, "error", "OpenCode bridge stopped. Restart the app to retry.");
        return false;
    }
    true
}

/// Select a chat and request its server state without blocking Slint.
pub fn open(ui: &AppWindow, shared: &Rc<RefCell<Shared>>, chat: &Chat) {
    ui.set_active_id(chat.id as i32);
    ui.set_active_title(chat.title.clone().into());
    ui.set_active_started(chat.started);
    ui.set_active_project(chat.project_id as i32);
    let project = shared.borrow().store.as_ref().and_then(|store| store.projects().ok())
        .and_then(|projects| projects.into_iter().find(|project| project.id == chat.project_id));
    ui.set_active_project_name(project.as_ref().map(|project| project.name.as_str()).unwrap_or("").into());
    ui.set_busy(shared.borrow().busy.contains(&chat.id));
    ui.set_current_model("".into());
    ui.set_current_level("".into());
    ui.set_permission_text("".into());
    ui.set_form_text("".into());
    ui.set_thinking_levels(ModelRc::new(VecModel::<slint::SharedString>::default()));
    ui.set_messages(ModelRc::new(VecModel::<ChatLine>::default()));
    // Reuse cached models so new windows feel instant.
    if !shared.borrow().models.is_empty() {
        let models = shared.borrow().models.clone();
        ui.set_all_models(ModelRc::new(VecModel::from(models.into_iter().map(Into::into).collect::<Vec<_>>())));
        ui.invoke_search_models("".into());
    }
    let directory = project.map(|project| project.path).unwrap_or_else(|| shared.borrow().directory.clone());
    shared.borrow_mut().ready.remove(&chat.id);
    let shared_ref = shared.borrow();
    send(ui, &shared_ref, json!({ "kind": "open", "id": chat.id, "session": chat.session, "title": chat.title, "directory": directory }));
}

/// Build, show, and register one window without touching the backend.
pub fn create_window(shared: &Rc<RefCell<Shared>>, _directory: String) -> Result<slint::Weak<AppWindow>, Box<dyn Error>> {
    let ui = AppWindow::new()?;
    // Fast shell: empty lists paint before SQLite or Node start.
    ui.set_messages(ModelRc::new(VecModel::<ChatLine>::default()));
    ui.set_sessions(ModelRc::new(VecModel::<ChatRow>::default()));
    ui.set_projects(ModelRc::new(VecModel::<ProjectRow>::default()));
    ui.set_active_id(-1);
    let weak = ui.as_weak();
    ui.on_minimize_window(move || {
        if let Some(ui) = weak.upgrade() {
            ui.window().set_minimized(true);
        }
    });
    let weak = ui.as_weak();
    ui.on_toggle_maximize_window(move || {
        if let Some(ui) = weak.upgrade() {
            let maximized = !ui.window().is_maximized();
            ui.window().set_maximized(maximized);
            ui.set_window_maximized(maximized);
        }
    });
    // In-process window: reuse the same bridge and store.
    let s = shared.clone();
    ui.on_new_window(move || {
        let dir = s.borrow().directory.clone();
        if let Ok(weak) = create_window(&s, dir) {
            if s.borrow().store.is_some() {
                if let Some(next) = weak.upgrade() {
                    chats::open_first(&next, &s);
                }
            }
        }
    });
    #[cfg(windows)]
    {
        use slint::winit_030::WinitWindowAccessor;
        let weak = ui.as_weak();
        ui.window().on_winit_window_event(move |window, event| {
            if let slint::winit_030::winit::event::WindowEvent::Resized(_) = event {
                if let Some(ui) = weak.upgrade() {
                    ui.set_window_maximized(window.is_maximized());
                }
            }
            slint::winit_030::EventResult::Propagate
        });
        let weak = ui.as_weak();
        slint::spawn_local(async move {
            if let Some(ui) = weak.upgrade() {
                if ui.window().winit_window().await.is_ok() {
                    if let Err(error) = crate::window::install(&ui) {
                        eprintln!("Window chrome: {error}");
                    }
                }
            }
        })?;
    }
    events::connect(&ui, shared);
    chats::wire(&ui, shared);
    if let Some(store) = &shared.borrow().store {
        if let Ok(zoom) = store.zoom() {
            ui.set_zoom(zoom);
        }
    }
    ui.show()?;
    let weak = keep(ui);
    shared.borrow().windows.lock().unwrap().push(weak.clone());
    shared.borrow().windows.lock().unwrap().retain(|w| w.upgrade().is_some());
    refresh_projects(&shared.borrow());
    Ok(weak)
}

/// Open SQLite off the UI thread so even a slow disk cannot stall first paint.
pub fn ensure_backend(shared: Rc<RefCell<Shared>>) {
    if shared.borrow().bridge.is_some() {
        return;
    }
    std::thread::spawn(move || {
        let store = Store::open().map_err(|error| error.to_string());
        let _ = slint::invoke_from_event_loop(move || finish_backend(store));
    });
}

/// Attach local metadata and start the bridge after SQLite is ready.
fn finish_backend(result: Result<Store, String>) {
    let Some(shared) = APP.with(|slot| slot.borrow().clone()) else { return };
    // Report startup failures inside the already visible window.
    match result {
        Ok(store) => {
            if let Err(error) = store.migrate_projects(std::path::Path::new(&shared.borrow().directory)) {
                for weak in shared.borrow().windows.lock().unwrap().iter() {
                    if let Some(ui) = weak.upgrade() {
                        append(&ui, "error", &format!("Cannot initialize projects: {error}"));
                    }
                }
                return;
            }
            if let Ok(zoom) = store.zoom() {
                for weak in shared.borrow().windows.lock().unwrap().iter() {
                    if let Some(ui) = weak.upgrade() {
                        ui.set_zoom(zoom);
                    }
                }
            }
            shared.borrow_mut().store = Some(store);
            refresh_projects(&shared.borrow());
        }
        Err(message) => {
            for weak in shared.borrow().windows.lock().unwrap().iter() {
                if let Some(ui) = weak.upgrade() {
                    append(&ui, "error", &format!("Cannot open local data: {message}"));
                }
            }
            return;
        }
    }
    // Start one bridge for all windows and broadcast to each shell.
    let directory = shared.borrow().directory.clone();
    let targets = shared.borrow().windows.clone();
    let bridge = opencode::start(directory, move |id, kind: String, data: Value| {
        for weak in targets.lock().unwrap().iter() {
            let kind = kind.clone();
            let data = data.clone();
            let _ = weak.upgrade_in_event_loop(move |ui| {
                ui.invoke_opencode_event(id as i32, kind.into(), data.to_string().into());
            });
        }
    });
    shared.borrow_mut().bridge = Some(bridge);
    // Fill every shell that opened before the backend was ready.
    let windows: Vec<_> = shared.borrow().windows.lock().unwrap().iter().filter_map(|w| w.upgrade()).collect();
    for ui in windows {
        chats::open_first(&ui, &shared);
    }
    drain_requests();
}
