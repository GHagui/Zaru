//! Small thumbnails, decoded once and kept on disk.
//!
//! This is the one place Zaru decodes an image in Rust, and the exception is
//! deliberate. The rule against it protects the culling loop, where a single
//! full-resolution frame has to appear inside one screen refresh and any work
//! on the critical path shows up as stutter. The grid is the opposite problem:
//! a hundred small images at once, none of them full size, and all of the work
//! can happen before anyone looks. Different physics, different answer.
//!
//! What it buys, measured on the fixture: the embedded `PRVW` preview is
//! 1620x1080 and 114 KB, and the WebView spends about 8 ms decoding each tile,
//! every time, including the next time the folder is opened. The cached
//! thumbnail is 810x540 and 21 KB, so the WebView decodes a quarter of the
//! pixels, and the second visit decodes nothing new at all.

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::SystemTime;

/// Long edge asked of the decoder. What comes back is the nearest rung of the
/// JPEG scaling ladder at or above it — see [`shrink`].
const TARGET_EDGE: u16 = 512;

const QUALITY: u8 = 82;

/// How large the cache folder may grow before the oldest files are dropped.
/// About twenty-five thousand thumbnails, and a cache that grows without a
/// ceiling in a temp folder is a trap rather than a feature.
pub const CACHE_LIMIT_BYTES: u64 = 512 * 1024 * 1024;

/// Where one photo's embedded preview lives, and what identifies it.
#[derive(Clone)]
pub struct Source {
    pub path: PathBuf,
    pub offset: u64,
    pub len: u64,
    /// Identity of the file's *content*, so a replaced file gets a new key and
    /// the stale thumbnail is simply never asked for again.
    pub key: String,
}

impl Source {
    /// Builds the cache key from what changes when a file changes: where it is,
    /// when it was last written, and how big it is.
    pub fn key_for(path: &Path, offset: u64, len: u64) -> String {
        let meta = fs::metadata(path).ok();
        let modified = meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);

        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        let mut eat = |bytes: &[u8]| {
            for b in bytes {
                hash = (hash ^ *b as u64).wrapping_mul(0x0000_0100_0000_01b3);
            }
        };
        eat(path.to_string_lossy().as_bytes());
        eat(&modified.to_le_bytes());
        eat(&size.to_le_bytes());
        eat(&offset.to_le_bytes());
        eat(&len.to_le_bytes());
        format!("{hash:016x}")
    }
}

type Responder = Box<dyn FnOnce(Option<Arc<Vec<u8>>>) + Send>;

struct State {
    sources: Arc<Vec<Source>>,
    /// Bumped on every folder change, so work in flight for the old folder is
    /// dropped instead of answering for the new one.
    generation: u64,
    memory: HashMap<usize, Arc<Vec<u8>>>,
    /// What the grid is waiting on right now.
    urgent: VecDeque<(usize, Responder)>,
    /// What is being built ahead of being asked for.
    background: VecDeque<usize>,
    shutdown: bool,
}

struct Inner {
    dir: PathBuf,
    state: Mutex<State>,
    work: Condvar,
}

pub struct Thumbs {
    inner: Arc<Inner>,
}

impl Thumbs {
    /// Starts the pool over `dir`.
    ///
    /// Every core gets a worker. Each one holds a single decoded preview —
    /// about 1.3 MB thanks to the scaled decode — so the whole pool costs a few
    /// tens of megabytes even on a large machine, and eighteen hundred photos
    /// warm in a couple of seconds.
    pub fn new(dir: PathBuf) -> Self {
        let inner = Arc::new(Inner {
            dir,
            state: Mutex::new(State {
                sources: Arc::new(Vec::new()),
                generation: 0,
                memory: HashMap::new(),
                urgent: VecDeque::new(),
                background: VecDeque::new(),
                shutdown: false,
            }),
            work: Condvar::new(),
        });

        let workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
        for _ in 0..workers {
            let inner = Arc::clone(&inner);
            std::thread::spawn(move || worker(inner));
        }
        Thumbs { inner }
    }

    /// The folder a temporary cache belongs in.
    ///
    /// `temp_dir` is `/tmp` on Linux and the per-user Temp folder under
    /// AppData on Windows, so one call lands in the right place on both with
    /// no branch to get wrong.
    pub fn default_dir() -> PathBuf {
        std::env::temp_dir().join("zaru").join("thumbs")
    }

    /// Points the pool at a new set of photos and queues them all for building.
    pub fn load(&self, sources: Vec<Source>) {
        {
            let mut state = self.inner.state.lock().unwrap();
            state.sources = Arc::new(sources);
            state.generation += 1;
            state.memory.clear();
            state.urgent.clear();
            state.background = (0..state.sources.len()).collect();
        }
        self.inner.work.notify_all();

        // Trimming once per folder, off the pool, keeps a file-system walk out
        // of the way of the thumbnails people are waiting for.
        let dir = self.inner.dir.clone();
        std::thread::spawn(move || sweep(&dir, CACHE_LIMIT_BYTES));
    }

    /// Reorders the queue so these are built first. The grid calls this as it
    /// scrolls; everything stays queued, only the order changes.
    pub fn prioritise(&self, wanted: &[usize]) {
        let mut state = self.inner.state.lock().unwrap();
        let ahead: Vec<usize> = wanted
            .iter()
            .copied()
            .filter(|i| *i < state.sources.len() && !state.memory.contains_key(i))
            .collect();
        for i in ahead.iter().rev() {
            state.background.retain(|q| q != i);
            state.background.push_front(*i);
        }
        drop(state);
        self.inner.work.notify_all();
    }

    /// Hands over one thumbnail: from memory if it is there, from disk or from
    /// a fresh decode otherwise.
    pub fn fetch(&self, index: usize, respond: Responder) {
        let mut state = self.inner.state.lock().unwrap();
        if index >= state.sources.len() {
            drop(state);
            respond(None);
            return;
        }
        if let Some(bytes) = state.memory.get(&index) {
            let bytes = Arc::clone(bytes);
            drop(state);
            respond(Some(bytes));
            return;
        }
        state.urgent.push_back((index, respond));
        drop(state);
        self.inner.work.notify_one();
    }

    /// Stores a thumbnail somebody else produced.
    ///
    /// Video is the case: Rust holds no video decoder, so the WebView draws a
    /// frame and hands the bytes back here. From the next session on it is an
    /// ordinary cache hit like any photo.
    pub fn store(&self, index: usize, bytes: Vec<u8>) -> std::io::Result<()> {
        let (dir, key) = {
            let state = self.inner.state.lock().unwrap();
            let Some(source) = state.sources.get(index) else {
                return Ok(());
            };
            (self.inner.dir.clone(), source.key.clone())
        };
        write_atomic(&dir, &key, &bytes)?;

        let mut state = self.inner.state.lock().unwrap();
        state.memory.insert(index, Arc::new(bytes));
        Ok(())
    }

    /// Which thumbnails are in memory, sorted. For tests and diagnostics.
    pub fn cached(&self) -> Vec<usize> {
        let state = self.inner.state.lock().unwrap();
        let mut out: Vec<usize> = state.memory.keys().copied().collect();
        out.sort_unstable();
        out
    }
}

impl Drop for Thumbs {
    fn drop(&mut self) {
        let mut state = self.inner.state.lock().unwrap();
        state.shutdown = true;
        drop(state);
        self.inner.work.notify_all();
    }
}

fn worker(inner: Arc<Inner>) {
    loop {
        let (index, respond, source, generation) = {
            let mut state = inner.state.lock().unwrap();
            loop {
                if state.shutdown {
                    return;
                }
                // Somebody waiting always beats somebody who will wait later.
                if let Some((index, respond)) = state.urgent.pop_front() {
                    let source = state.sources.get(index).cloned();
                    break (index, Some(respond), source, state.generation);
                }
                if let Some(index) = state.background.pop_front() {
                    if state.memory.contains_key(&index) {
                        continue;
                    }
                    let source = state.sources.get(index).cloned();
                    break (index, None, source, state.generation);
                }
                state = inner.work.wait(state).unwrap();
            }
        };

        let bytes = source.and_then(|s| build(&inner.dir, &s).ok()).map(Arc::new);

        if let Some(bytes) = &bytes {
            let mut state = inner.state.lock().unwrap();
            // The folder may have changed while this one was being built.
            if state.generation == generation {
                state.memory.insert(index, Arc::clone(bytes));
            }
        }
        if let Some(respond) = respond {
            respond(bytes);
        }
    }
}

/// Reads the cached thumbnail, or makes it and caches it.
fn build(dir: &Path, source: &Source) -> std::io::Result<Vec<u8>> {
    let file = cache_path(dir, &source.key);
    if let Ok(bytes) = fs::read(&file) {
        if !bytes.is_empty() {
            return Ok(bytes);
        }
    }

    let jpeg = read_range(source)?;
    let thumbnail = shrink(&jpeg)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    write_atomic(dir, &source.key, &thumbnail)?;
    Ok(thumbnail)
}

fn read_range(source: &Source) -> std::io::Result<Vec<u8>> {
    let mut file = fs::File::open(&source.path)?;
    file.seek(SeekFrom::Start(source.offset))?;
    let mut buf = vec![0u8; source.len as usize];
    file.read_exact(&mut buf)?;
    Ok(buf)
}

/// Decodes at a reduced size and re-encodes small.
///
/// The decoder scales during the inverse DCT, so the full image is never
/// built: a 1620x1080 preview comes out at 810x540 for about 1.3 MB instead of
/// 5 MB, and in roughly half the time. The ladder only has the rungs 1/1, 1/2,
/// 1/4 and 1/8, and the result is used as it lands rather than resampled to an
/// exact width — a resampler would cost more than the few kilobytes it saves.
fn shrink(jpeg: &[u8]) -> Result<Vec<u8>, String> {
    let mut decoder = jpeg_decoder::Decoder::new(jpeg);
    decoder.read_info().map_err(|e| e.to_string())?;
    let (width, height) = decoder
        .scale(TARGET_EDGE, TARGET_EDGE)
        .map_err(|e| e.to_string())?;
    let pixels = decoder.decode().map_err(|e| e.to_string())?;

    let colour = match decoder.info().map(|i| i.pixel_format) {
        Some(jpeg_decoder::PixelFormat::L8) => jpeg_encoder::ColorType::Luma,
        Some(jpeg_decoder::PixelFormat::RGB24) => jpeg_encoder::ColorType::Rgb,
        other => return Err(format!("unsupported pixel format: {other:?}")),
    };

    let mut out = Vec::with_capacity(32 * 1024);
    jpeg_encoder::Encoder::new(&mut out, QUALITY)
        .encode(&pixels, width, height, colour)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

fn cache_path(dir: &Path, key: &str) -> PathBuf {
    dir.join(format!("{key}.jpg"))
}

fn write_atomic(dir: &Path, key: &str, bytes: &[u8]) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    // Named per worker: several threads write into this folder at once, and a
    // shared temporary name would let one truncate another's file.
    let tmp = dir.join(format!("{key}.{:?}.tmp", std::thread::current().id()));
    fs::write(&tmp, bytes)?;
    match fs::rename(&tmp, cache_path(dir, key)) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// Drops the least recently touched files until the folder fits under `limit`.
pub fn sweep(dir: &Path, limit: u64) -> u64 {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    let mut files: Vec<(SystemTime, u64, PathBuf)> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            if !meta.is_file() {
                return None;
            }
            Some((meta.modified().ok()?, meta.len(), e.path()))
        })
        .collect();

    let mut total: u64 = files.iter().map(|(_, size, _)| size).sum();
    if total <= limit {
        return total;
    }

    files.sort_by_key(|(when, _, _)| *when);
    for (_, size, path) in files {
        if total <= limit {
            break;
        }
        if fs::remove_file(&path).is_ok() {
            total -= size;
        }
    }
    total
}

