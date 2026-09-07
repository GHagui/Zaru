#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Zaru: keyboard-driven culling for Canon CR3 photos.
//!
//! The one rule the rest of the design follows from: a preview never travels
//! through the command channel. Serialising a megabyte of JPEG into a JSON
//! string, per keypress, is exactly the latency this app exists to remove. The
//! bytes go over a custom URI scheme instead, so the front end is an `<img>`
//! tag and the WebView's own image pipeline does the work.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use tauri::{Manager, State};
use tauri_plugin_dialog::DialogExt;

use zaru_core::keymap::ACTIONS;
use zaru_core::{
    ApplyOperation, BatchEdit, ApplyPlan, Exif, Frame, Keymap, ThumbSource, Thumbs, PhotoChange, Prefetch, Recovery, RecoveryOffer, Session,
    SessionView, Settings,
};

/// The colour Zaru puts on a photo. `xmp:Label` holds one colour per photo,
/// and green is the only one the keyboard reaches.
const LABEL: &str = "Green";

struct AppState {
    session: Mutex<Session>,
    prefetch: Prefetch,
    thumbnails: Thumbs,
    settings: Mutex<Settings>,
    config_dir: PathBuf,
    /// Set by anything that changes a mark, cleared by a checkpoint. Saving on
    /// every keystroke would put a file write in the culling loop.
    dirty: AtomicBool,
}

impl AppState {
    fn new(config_dir: PathBuf) -> Self {
        AppState {
            session: Mutex::new(Session::default()),
            prefetch: Prefetch::new(),
            thumbnails: Thumbs::new(Thumbs::default_dir()),
            settings: Mutex::new(Settings::load(&config_dir)),
            config_dir,
            dirty: AtomicBool::new(false),
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
    if let Err(error) = session.open(Path::new(&path)) {
        let recovered = Recovery::load(&state.config_dir, Path::new(&path))
            .map(|saved| session.open_recoverable(Path::new(&path), &saved)).unwrap_or(false);
        if !recovered { return Err(error); }
    }
    reload_frames(&state, &session);
    state.dirty.store(false, Ordering::Relaxed);
    Ok(session.view())
}

/// Points the prefetch pool at the session's current paths. Called on open, and
/// again after Apply, because a photo that moved into a collection lives
/// somewhere else now.
fn reload_frames(state: &AppState, session: &Session) {
    state.thumbnails.load(session.photos().iter().map(|photo| {
        // A video has no embedded preview, so its source is empty: only a
        // thumbnail the viewer drew and handed back can satisfy it.
        let preview = photo.info.thumbnail_source();
        let (offset, len) = preview.map(|p| (p.offset, p.len)).unwrap_or((0, 0));
        ThumbSource {
            key: ThumbSource::key_for(&photo.path, offset, len),
            path: photo.path.clone(),
            offset,
            len,
        }
    }).collect());
    state.prefetch.load(
        session
            .photos()
            .iter()
            .map(|p| {
                let preview = p.info.preview;
                Frame {
                    path: p.path.clone(),
                    offset: preview.map(|v| v.offset).unwrap_or(0),
                    len: preview.map(|v| v.len).unwrap_or(0),
                }
            })
            .collect(),
    );
}

/// Navigation itself never crosses this boundary — the front end swaps between
/// images it already holds. This only says which frames to keep ready.
///
/// The front end sends the frames rather than a position, because with a filter
/// on, "the next five photos" is a walk through a subset and the neighbours in
/// the file list are not the neighbours in the pass.
#[tauri::command]
fn focus(state: State<'_, AppState>, frames: Vec<usize>) {
    state.prefetch.focus(&frames);
}

#[tauri::command]
fn thumbnail_focus(state: State<'_, AppState>, frames: Vec<usize>) {
    // Everything stays queued whatever the grid asks for; only the order
    // changes, so scrolling past a photo does not cancel building its
    // thumbnail — it just stops being first in line.
    state.thumbnails.prioritise(&frames);
}

/// Files a thumbnail the front end produced.
///
/// Rust holds no video decoder, so for a video the WebView draws a frame and
/// hands the bytes back here. From the next session on it is an ordinary cache
/// hit like any photo.
#[tauri::command]
fn cache_thumbnail(state: State<'_, AppState>, index: usize, bytes: Vec<u8>) {
    let _ = state.thumbnails.store(index, bytes);
}

#[tauri::command]
fn edit_selection(state: State<'_, AppState>, indices: Vec<usize>, edit: BatchEdit) -> Result<Vec<PhotoChange>, String> {
    let changes = state.session.lock().unwrap().edit_selection(&indices, edit)?;
    Ok(touched(&state, changes))
}

#[tauri::command]
fn set_star(state: State<'_, AppState>, index: usize, stars: i8) -> Option<PhotoChange> {
    changed(&state, state.session.lock().unwrap().set_star(index, stars))
}

#[tauri::command]
fn toggle_reject(state: State<'_, AppState>, index: usize) -> Option<PhotoChange> {
    changed(&state, state.session.lock().unwrap().toggle_reject(index))
}

#[tauri::command]
fn toggle_label(state: State<'_, AppState>, index: usize) -> Option<PhotoChange> {
    changed(&state, state.session.lock().unwrap().toggle_label(index, LABEL))
}

#[tauri::command]
fn undo(state: State<'_, AppState>) -> Vec<PhotoChange> {
    touched(&state, state.session.lock().unwrap().undo())
}

#[tauri::command]
fn redo(state: State<'_, AppState>) -> Vec<PhotoChange> {
    touched(&state, state.session.lock().unwrap().redo())
}

fn changed<T>(state: &AppState, outcome: Option<T>) -> Option<T> {
    if outcome.is_some() {
        state.dirty.store(true, Ordering::Relaxed);
    }
    outcome
}

fn touched(state: &AppState, changes: Vec<PhotoChange>) -> Vec<PhotoChange> {
    if !changes.is_empty() {
        state.dirty.store(true, Ordering::Relaxed);
    }
    changes
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

/// Registers a collection. Nothing is created on disk until Apply, so a session
/// the user walks away from leaves no empty folders behind.
#[tauri::command]
fn new_collection(state: State<'_, AppState>, name: String) -> Result<Vec<String>, String> {
    // The key map is the cap: a collection no key reaches is not worth having.
    let limit = state.settings.lock().unwrap().keymap.collections.len();
    let mut session = state.session.lock().unwrap();
    session.new_collection(&name, limit)?;
    state.dirty.store(true, Ordering::Relaxed);
    Ok(session.collections().to_vec())
}

#[tauri::command]
fn assign(
    state: State<'_, AppState>,
    index: usize,
    collection: Option<usize>,
) -> Option<PhotoChange> {
    changed(&state, state.session.lock().unwrap().assign(index, collection))
}

/// Sends the whole burst at `index` to one collection, as one undoable step.
#[tauri::command]
fn assign_burst(
    state: State<'_, AppState>,
    index: usize,
    collection: Option<usize>,
) -> Vec<PhotoChange> {
    touched(&state, state.session.lock().unwrap().assign_burst(index, collection))
}

/// What the camera recorded about one frame.
///
/// Per photo rather than in the session view: these fields only matter while
/// the panel showing them is open, and multiplying ten of them by eighteen
/// hundred photos in the opening payload would be paying for nothing.
#[tauri::command]
fn exif(state: State<'_, AppState>, index: usize) -> Option<Exif> {
    state.session.lock().unwrap().exif(index)
}

/// Every rebindable action, with the label the settings screen shows.
#[tauri::command]
fn key_actions() -> Vec<(String, String)> {
    ACTIONS
        .iter()
        .map(|(id, label)| ((*id).to_string(), (*label).to_string()))
        .collect()
}

/// Moves one action onto one key, refusing anything that would leave two
/// actions sharing it.
#[tauri::command]
fn bind_key(state: State<'_, AppState>, action: String, key: String) -> Result<Keymap, String> {
    let mut settings = state.settings.lock().unwrap();
    settings.keymap.set(&action, &key)?;
    settings
        .save(&state.config_dir)
        .map_err(|e| format!("não deu para salvar: {e}"))?;
    Ok(settings.keymap.clone())
}

#[tauri::command]
fn reset_keymap(state: State<'_, AppState>) -> Result<Keymap, String> {
    let mut settings = state.settings.lock().unwrap();
    settings.keymap = Keymap::default();
    settings
        .save(&state.config_dir)
        .map_err(|e| format!("não deu para salvar: {e}"))?;
    Ok(settings.keymap.clone())
}

/// What the last session left behind for this folder, if it still fits.
#[tauri::command]
fn recovery_offer(state: State<'_, AppState>) -> Option<RecoveryOffer> {
    let session = state.session.lock().unwrap();
    Recovery::load(&state.config_dir, session.folder())
        .filter(|saved| saved.valid_for(session.folder()))
        .map(|saved| saved.offer())
}

#[tauri::command]
fn restore_session(state: State<'_, AppState>) -> Option<SessionView> {
    let mut session = state.session.lock().unwrap();
    let saved = Recovery::load(&state.config_dir, session.folder())?;
    if !session.restore(saved) { return None; }
    reload_frames(&state, &session);
    Some(session.view())
}

#[tauri::command]
fn discard_recovery(state: State<'_, AppState>) {
    let session = state.session.lock().unwrap();
    Recovery::discard(&state.config_dir, session.folder());
}

/// Writes the scratch copy if anything has changed since the last one.
///
/// Called on a timer by the front end rather than after every keystroke: two
/// thousand marks is a hundred kilobytes, and putting that write in the culling
/// loop is exactly the kind of latency this app exists to avoid.
#[tauri::command]
fn checkpoint(state: State<'_, AppState>) {
    if !state.dirty.swap(false, Ordering::Relaxed) {
        return;
    }
    let session = state.session.lock().unwrap();
    if !session.has_work() {
        return;
    }
    if session.snapshot().save(&state.config_dir).is_err() {
        // Nothing to tell the user: the marks are still in memory, and the next
        // checkpoint tries again.
        state.dirty.store(true, Ordering::Relaxed);
    }
}

/// What Apply would do. The preview is not decoration: moving files is the only
/// irreversible thing Zaru does, and it should never be a surprise.
#[tauri::command]
fn plan(state: State<'_, AppState>, indices: Option<Vec<usize>>, operation: Option<ApplyOperation>) -> Result<ApplyPlan, String> {
    let styles = state.settings.lock().unwrap().xmp_compat.styles();
    state.session.lock().unwrap().plan_selection(indices.as_deref(), operation.unwrap_or_default(), styles)
}

#[tauri::command]
fn apply(state: State<'_, AppState>, indices: Option<Vec<usize>>, operation: Option<ApplyOperation>) -> Result<serde_json::Value, String> {
    let styles = state.settings.lock().unwrap().xmp_compat.styles();
    let mut session = state.session.lock().unwrap();
    let report = session.apply_selection(indices.as_deref(), operation.unwrap_or_default(), styles)?;
    // Some photos live in a subfolder now, so the cached byte ranges point at
    // paths that no longer exist.
    reload_frames(&state, &session);
    // The marks are on disk in their real home; the scratch copy has no job.
    if !session.has_pending() && report.error.is_none() {
        Recovery::discard(&state.config_dir, session.folder());
        state.dirty.store(false, Ordering::Relaxed);
    } else if session.snapshot().save(&state.config_dir).is_err() {
        state.dirty.store(true, Ordering::Relaxed);
    }
    let mut value = serde_json::to_value(report).map_err(|e| e.to_string())?;
    value["session"] = serde_json::to_value(session.view()).map_err(|e| e.to_string())?;
    Ok(value)
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

            // A video is served from the file itself, in slices.
            if request.uri().path().contains("/media/") {
                let path = state.session.lock().unwrap().media_path(index);
                let range = request
                    .headers()
                    .get("range")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string);
                responder.respond(match path {
                    Some(path) => ranged(&path, mime_for(&path), range.as_deref()),
                    None => not_found(),
                });
                return;
            }

            let answer = move |bytes: Option<std::sync::Arc<Vec<u8>>>| {
                responder.respond(match bytes {
                    Some(bytes) => jpeg(bytes.as_ref().clone()),
                    None => not_found(),
                })
            };
            if request.uri().path().contains("/thumb/") {
                state.thumbnails.fetch(index, Box::new(answer));
            } else {
                state.prefetch.fetch(index, Box::new(answer));
            }
        })
        .invoke_handler(tauri::generate_handler![
            pick_folder,
            open_folder,
            focus,
            thumbnail_focus,
            cache_thumbnail,
            edit_selection,
            set_star,
            toggle_reject,
            toggle_label,
            undo,
            redo,
            get_settings,
            set_settings,
            exif,
            key_actions,
            bind_key,
            reset_keymap,
            new_collection,
            assign,
            assign_burst,
            plan,
            apply,
            recovery_offer,
            restore_session,
            discard_recovery,
            checkpoint,
        ])
        .run(tauri::generate_context!())
        .expect("Zaru failed to start");
}

/// `zaru://localhost/42` on most platforms, `http://zaru.localhost/42` on
/// Windows. Either way the index is the last path segment.
fn frame_index(path: &str) -> Option<usize> {
    path.rsplit('/').next()?.parse().ok()
}

/// Serves a file, honouring a `Range` request.
///
/// Without this a `<video>` cannot seek at all — the element needs `206` and a
/// `Content-Range` to jump — and every request would pull the whole file into
/// memory, which for a few minutes of 4K is gigabytes.
fn ranged(path: &std::path::Path, mime: &str, range: Option<&str>) -> tauri::http::Response<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};

    let Ok(mut file) = std::fs::File::open(path) else {
        return not_found();
    };
    let Ok(total) = file.metadata().map(|m| m.len()) else {
        return not_found();
    };

    let wanted = zaru_core::media::range_slice(total, range);
    let (start, end) = wanted.unwrap_or((0, total.saturating_sub(1)));
    let length = end.saturating_sub(start) + 1;

    let mut body = vec![0u8; length as usize];
    if file.seek(SeekFrom::Start(start)).is_err() || file.read_exact(&mut body).is_err() {
        return not_found();
    }

    let partial = wanted.is_some();
    tauri::http::Response::builder()
        .status(if partial { 206 } else { 200 })
        .header("Content-Type", mime)
        .header("Accept-Ranges", "bytes")
        .header("Content-Length", length.to_string())
        .header("Cache-Control", "no-store")
        .header("Content-Range", format!("bytes {start}-{end}/{total}"))
        .body(body)
        .unwrap_or_else(|_| not_found())
}

fn mime_for(path: &std::path::Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase) {
        Some(ext) if ext == "mov" => "video/quicktime",
        _ => "video/mp4",
    }
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
