//! Keeps the previews around the current photo in memory.
//!
//! Extracting a preview is a seek and a read — there is no decoding on this
//! side — so what this buys is not CPU but the disk round trip, and it warms
//! the OS page cache for the frames the user is about to reach. The decode
//! itself happens in the WebView, and the front end hides that behind its own
//! ring of already-decoded images.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};

const WORKERS: usize = 4;

#[derive(Clone)]
pub struct Frame {
    pub path: PathBuf,
    pub offset: u64,
    pub len: u64,
}

type Responder = Box<dyn FnOnce(Option<Arc<Vec<u8>>>) + Send>;

struct State {
    frames: Arc<Vec<Frame>>,
    /// Bumped on every folder change, so work queued for the old folder is
    /// dropped instead of landing in the new folder's cache.
    generation: u64,
    cache: HashMap<usize, Arc<Vec<u8>>>,
    /// The previous window, kept one round longer so reversing direction does
    /// not re-read everything that was just evicted.
    previous: Vec<usize>,
    warm: VecDeque<usize>,
    serve: VecDeque<(usize, Responder)>,
    shutdown: bool,
}

struct Inner {
    state: Mutex<State>,
    work: Condvar,
}

pub struct Prefetch {
    inner: Arc<Inner>,
}

impl Prefetch {
    pub fn new() -> Self {
        let inner = Arc::new(Inner {
            state: Mutex::new(State {
                frames: Arc::new(Vec::new()),
                generation: 0,
                cache: HashMap::new(),
                previous: Vec::new(),
                warm: VecDeque::new(),
                serve: VecDeque::new(),
                shutdown: false,
            }),
            work: Condvar::new(),
        });

        for _ in 0..WORKERS {
            let inner = Arc::clone(&inner);
            std::thread::spawn(move || worker(inner));
        }

        Prefetch { inner }
    }

    /// Points the pool at a new set of frames and drops everything cached for
    /// the previous one.
    pub fn load(&self, frames: Vec<Frame>) {
        let mut state = self.inner.state.lock().unwrap();
        state.frames = Arc::new(frames);
        state.generation += 1;
        state.cache.clear();
        state.previous.clear();
        state.warm.clear();
        let abandoned: Vec<_> = state.serve.drain(..).collect();
        drop(state);
        for (_, respond) in abandoned { respond(None); }
    }

    /// Sets the window to exactly these frames, in this order of urgency.
    ///
    /// The caller decides which frames those are, rather than this pool
    /// inferring them from an index. That matters as soon as a filter is on:
    /// "the next five photos" is then a walk through a subset, and neighbours
    /// in the file list are not neighbours in the pass.
    pub fn focus(&self, wanted: &[usize]) {
        let mut state = self.inner.state.lock().unwrap();
        if state.frames.is_empty() {
            return;
        }

        let keep: HashSet<usize> = wanted.iter().chain(&state.previous).copied().collect();
        state.cache.retain(|i, _| keep.contains(i));

        // Whatever was queued was queued for a window that has moved.
        state.warm.clear();
        for i in wanted {
            if *i < state.frames.len() && !state.cache.contains_key(i) {
                state.warm.push_back(*i);
            }
        }
        state.previous = wanted.to_vec();
        drop(state);
        self.inner.work.notify_all();
    }

    /// Which frames are currently in memory, sorted. For tests and diagnostics.
    pub fn cached(&self) -> Vec<usize> {
        let state = self.inner.state.lock().unwrap();
        let mut out: Vec<usize> = state.cache.keys().copied().collect();
        out.sort_unstable();
        out
    }

    /// Hands the bytes for `index` to `respond`, immediately on a cache hit and
    /// from a worker thread otherwise.
    pub fn fetch(&self, index: usize, respond: Responder) {
        let mut state = self.inner.state.lock().unwrap();
        if index >= state.frames.len() {
            drop(state);
            respond(None);
            return;
        }
        if let Some(bytes) = state.cache.get(&index) {
            let bytes = Arc::clone(bytes);
            drop(state);
            respond(Some(bytes));
            return;
        }
        state.serve.push_back((index, respond));
        drop(state);
        self.inner.work.notify_one();
    }
}

impl Default for Prefetch {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Prefetch {
    fn drop(&mut self) {
        let mut state = self.inner.state.lock().unwrap();
        state.shutdown = true;
        drop(state);
        self.inner.work.notify_all();
    }
}

fn worker(inner: Arc<Inner>) {
    loop {
        let (index, respond, frame, generation) = {
            let mut state = inner.state.lock().unwrap();
            loop {
                if state.shutdown {
                    return;
                }
                // A request the user is waiting on always beats speculation.
                if let Some((index, respond)) = state.serve.pop_front() {
                    let frame = state.frames.get(index).cloned();
                    break (index, Some(respond), frame, state.generation);
                }
                if let Some(index) = state.warm.pop_front() {
                    if state.cache.contains_key(&index) {
                        continue;
                    }
                    let frame = state.frames.get(index).cloned();
                    break (index, None, frame, state.generation);
                }
                state = inner.work.wait(state).unwrap();
            }
        };

        let bytes = frame.and_then(|f| read_range(&f).ok()).map(Arc::new);

        if let Some(bytes) = &bytes {
            let mut state = inner.state.lock().unwrap();
            // The folder may have changed while this read was in flight.
            if state.generation == generation {
                state.cache.insert(index, Arc::clone(bytes));
            }
        }

        if let Some(respond) = respond {
            respond(bytes);
        }
    }
}

fn read_range(frame: &Frame) -> std::io::Result<Vec<u8>> {
    let mut file = std::fs::File::open(&frame.path)?;
    file.seek(SeekFrom::Start(frame.offset))?;
    let mut buf = vec![0u8; frame.len as usize];
    file.read_exact(&mut buf)?;
    Ok(buf)
}
