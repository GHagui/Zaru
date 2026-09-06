#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Zaru: keyboard-driven culling for Canon CR3 photos.
//!
//! The one rule the rest of the design follows from: a preview never travels
//! through the command channel. Serialising a megabyte of JPEG into a JSON
//! string, per keypress, is exactly the latency this app exists to remove. The
//! bytes go over a custom URI scheme instead, so the front end is an `<img>`
//! tag and the WebView's own image pipeline does the work.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tauri::{Manager, State};
use tauri_plugin_dialog::DialogExt;

use zaru_core::{Frame, MarkChange, Prefetch, Session, SessionView, Settings, WriteReport};

/// The colour Zaru puts on a photo. `xmp:Label` holds one colour per photo,
/// and green is the only one the keyboard reaches.
const LABEL: &str = "Green";

struct AppState {
    session: Mutex<Session>,
    prefetch: Prefetch,
    settings: Mutex<Settings>,
    config_dir: PathBuf,
}

impl AppState {
    fn new(config_dir: PathBuf) -> Self {
        AppState {
            session: Mutex::new(Session::default()),
            prefetch: Prefetch::new(),
            settings: Mutex::new(Settings::load(&config_dir)),
            config_dir,
        }
    }
}

#[tauri::command]
async fn pick_folder(app: tauri::AppHandle) -> Option<String> {
    let (tx, rx) = std::sync::mpsc::channel();
    app.dialog().file().pick_folder(move |folder| {
        let _ = tx.send(folder);
    });
    rx.recv().ok().flatten().map(|f| f.to_string())
}

#[tauri::command]
fn open_folder(state: State<'_, AppState>, path: String) -> Result<SessionView, String> {
    let mut session = state.session.lock().unwrap();
    session.open(Path::new(&path))?;

    state.prefetch.load(
        session
            .photos()
            .iter()
            .map(|p| Frame {
                path: p.path.clone(),
                offset: p.info.preview.offset,
                len: p.info.preview.len,
            })
            .collect(),
    );
    state.prefetch.slide(0);

    Ok(session.view())
}

/// Navigation itself never crosses this boundary — the front end swaps between
/// images it already holds. This only tells the pool where the window is now.
#[tauri::command]
fn set_index(state: State<'_, AppState>, index: usize) {
    state.prefetch.slide(index);
}

#[tauri::command]
fn set_star(state: State<'_, AppState>, index: usize, stars: i8) -> Option<MarkChange> {
    state.session.lock().unwrap().set_star(index, stars)
}

#[tauri::command]
fn toggle_reject(state: State<'_, AppState>, index: usize) -> Option<MarkChange> {
    state.session.lock().unwrap().toggle_reject(index)
}

#[tauri::command]
fn toggle_label(state: State<'_, AppState>, index: usize) -> Option<MarkChange> {
    state.session.lock().unwrap().toggle_label(index, LABEL)
}

#[tauri::command]
fn undo(state: State<'_, AppState>) -> Option<MarkChange> {
    state.session.lock().unwrap().undo()
}

#[tauri::command]
fn redo(state: State<'_, AppState>) -> Option<MarkChange> {
    state.session.lock().unwrap().redo()
}

#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> Settings {
    state.settings.lock().unwrap().clone()
}

#[tauri::command]
fn set_settings(state: State<'_, AppState>, settings: Settings) -> Result<Settings, String> {
    let mut current = state.settings.lock().unwrap();
    *current = settings.clone();
    settings
        .save(&state.config_dir)
        .map_err(|e| format!("could not save settings: {e}"))?;
    Ok(settings)
}

#[tauri::command]
fn write_xmp(state: State<'_, AppState>) -> WriteReport {
    let styles = state.settings.lock().unwrap().xmp_compat.styles();
    state.session.lock().unwrap().write_xmp(styles)
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let config_dir = app
                .path()
                .app_config_dir()
                .unwrap_or_else(|_| PathBuf::from("."));
            app.manage(AppState::new(config_dir));
            Ok(())
        })
        .register_asynchronous_uri_scheme_protocol("zaru", |ctx, request, responder| {
            let state = ctx.app_handle().state::<AppState>();

            let Some(index) = frame_index(request.uri().path()) else {
                responder.respond(not_found());
                return;
            };

            state.prefetch.fetch(
                index,
                Box::new(move |bytes| {
                    responder.respond(match bytes {
                        Some(bytes) => jpeg(bytes.as_ref().clone()),
                        None => not_found(),
                    });
                }),
            );
        })
        .invoke_handler(tauri::generate_handler![
            pick_folder,
            open_folder,
            set_index,
            set_star,
            toggle_reject,
            toggle_label,
            undo,
            redo,
            get_settings,
            set_settings,
            write_xmp,
        ])
        .run(tauri::generate_context!())
        .expect("Zaru failed to start");
}

/// `zaru://localhost/42` on most platforms, `http://zaru.localhost/42` on
/// Windows. Either way the index is the last path segment.
fn frame_index(path: &str) -> Option<usize> {
    path.rsplit('/').next()?.parse().ok()
}

fn jpeg(bytes: Vec<u8>) -> tauri::http::Response<Vec<u8>> {
    tauri::http::Response::builder()
        .status(200)
        .header("Content-Type", "image/jpeg")
        // The WebView must not answer a later request for this URL from its
        // own cache: after an undo or a jump, that would paint the wrong photo.
        .header("Cache-Control", "no-store")
        .body(bytes)
        .expect("static response")
}

fn not_found() -> tauri::http::Response<Vec<u8>> {
    tauri::http::Response::builder()
        .status(404)
        .body(Vec::new())
        .expect("static response")
}
