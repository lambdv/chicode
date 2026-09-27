//! Load native files and clipboard images away from the UI thread.
use crate::{AppWindow, app};
use base64::{Engine, engine::general_purpose::STANDARD};
use slint::ComponentHandle;
use std::{io::Cursor, path::PathBuf};

/// Validate supported files and produce self-contained URI attachments.
fn load(path: PathBuf) -> Result<serde_json::Value, String> {
    let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let mime = match path.extension().and_then(|value| value.to_str()).unwrap_or("").to_lowercase().as_str() {
        "png" => "image/png", "jpg" | "jpeg" => "image/jpeg", "gif" => "image/gif", "webp" => "image/webp", "pdf" => "application/pdf",
        "txt" | "md" | "rs" | "js" | "ts" | "tsx" | "jsx" | "json" | "toml" | "yaml" | "yml" | "csv" | "py" | "html" | "css" | "xml" | "log" | "sh" | "sql" | "c" | "h" | "cpp" => "text/plain",
        _ => return Err(format!("Unsupported attachment: {name}")),
    };
    if std::fs::metadata(&path).map_err(|e| e.to_string())?.len() > 20 * 1024 * 1024 { return Err(format!("Attachment exceeds 20 MB: {name}")); }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if mime == "text/plain" && std::str::from_utf8(&bytes).is_err() { return Err(format!("Text attachment is not UTF-8: {name}")); }
    Ok(serde_json::json!({ "name": name, "mime": mime, "uri": format!("data:{mime};base64,{}", STANDARD.encode(bytes)) }))
}

/// Report an asynchronous result to the original session without touching the draft.
fn deliver(weak: slint::Weak<AppWindow>, id: i32, result: Result<serde_json::Value, String>) {
    let _ = weak.upgrade_in_event_loop(move |ui| {
        ui.invoke_attachment_loading(id, -1);
        match result {
            Ok(value) => ui.invoke_attachment_ready(id, value.to_string().into()),
            Err(error) => app::append(&ui, "error", &error),
        }
    });
}

/// Read dropped files asynchronously using the same validation as the picker.
pub fn dropped(ui: &AppWindow, path: PathBuf) {
    let weak = ui.as_weak(); let id = ui.get_active_id();
    ui.invoke_attachment_loading(id, 1);
    std::thread::spawn(move || deliver(weak, id, load(path)));
}

/// Wire native file picking and image clipboard fallback; text pastes stay in Slint.
pub fn wire(ui: &AppWindow) {
    let weak = ui.as_weak();
    ui.on_preview_attachment(move |data| {
        let weak = weak.clone();
        std::thread::spawn(move || {
            let result = (|| -> Result<(), String> {
                let file: serde_json::Value = serde_json::from_str(&data).map_err(|error| error.to_string())?;
                let bytes = STANDARD.decode(file["data"].as_str().unwrap_or("")).map_err(|error| error.to_string())?;
                if file["mime"].as_str().unwrap_or("").starts_with("image/") {
                    let image = image::load_from_memory(&bytes).map_err(|error| error.to_string())?.thumbnail(1400, 1000).to_rgba8();
                    let (width, height) = image.dimensions();
                    let bytes = image.into_raw();
                    let _ = weak.upgrade_in_event_loop(move |ui| {
                        let pixels = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&bytes, width, height);
                        ui.set_attachment_preview(slint::Image::from_rgba8(pixels)); ui.set_preview_visible(true);
                    });
                } else if let Some(path) = rfd::FileDialog::new().set_file_name(file["name"].as_str().unwrap_or("attachment")).save_file() {
                    std::fs::write(path, bytes).map_err(|error| error.to_string())?;
                }
                Ok(())
            })();
            if let Err(error) = result { let _ = weak.upgrade_in_event_loop(move |ui| app::append(&ui, "error", &error)); }
        });
    });
    let weak = ui.as_weak();
    ui.on_pick_attachments(move || {
        let Some(ui) = weak.upgrade() else { return };
        let weak = ui.as_weak(); let id = ui.get_active_id();
        std::thread::spawn(move || {
            if let Some(paths) = rfd::FileDialog::new().pick_files() {
                for path in paths {
                    let _ = weak.upgrade_in_event_loop(move |ui| ui.invoke_attachment_loading(id, 1));
                    deliver(weak.clone(), id, load(path));
                }
            }
        });
    });
    let weak = ui.as_weak();
    ui.on_paste_image(move || {
        let Some(ui) = weak.upgrade() else { return false };
        let Ok(mut clipboard) = arboard::Clipboard::new() else { return false };
        let Ok(data) = clipboard.get_image() else { return false };
        let (width, height, bytes) = (data.width, data.height, data.bytes.into_owned());
        let weak = ui.as_weak(); let id = ui.get_active_id();
        ui.invoke_attachment_loading(id, 1);
        std::thread::spawn(move || {
            let result = (|| {
                let image = image::RgbaImage::from_raw(width as u32, height as u32, bytes).ok_or("Invalid clipboard image")?;
                let mut buffer = Cursor::new(Vec::new());
                image.write_to(&mut buffer, image::ImageFormat::Png).map_err(|_| "Cannot encode clipboard image")?;
                Ok(serde_json::json!({ "name": "Clipboard.png", "mime": "image/png", "uri": format!("data:image/png;base64,{}", STANDARD.encode(buffer.into_inner())) }))
            })();
            deliver(weak, id, result.map_err(str::to_owned));
        });
        true
    });
}
