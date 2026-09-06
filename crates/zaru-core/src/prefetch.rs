//! Keeps the previews around the current photo in memory.
//!
//! Extracting a preview is a seek and a read — there is no decoding on this
//! side — so what this buys is not CPU but the disk round trip, and it warms
//! the OS page cache for the frames the user is about to reach. The decode
//! itself happens in the WebView, and the front end hides that behind its own
//! ring of already-decoded images.

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};

/// Frames kept ready ahead of and behind the current one.
const AHEAD: usize = 5;
const BEHIND: usize = 3;
/// Slack before a frame is dropped, so reversing direction does not re-read
/// everything that was just evicted.
const KEEP_AHEAD: usize = 10;
const KEEP_BEHIND: usize = 6;

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
        state.warm.clear();
        state.serve.clear();
    }

    /// Moves the window to `index`: evicts what fell out of it and queues what
    /// is missing, nearest frame first and forward before backward.
    pub fn slide(&self, index: usize) {
        let mut state = self.inner.state.lock().unwrap();
        let len = state.frames.len();
        if len == 0 {
            return;
        }

        let keep_lo = index.saturating_sub(KEEP_BEHIND);
        let keep_hi = (index + KEEP_AHEAD).min(len - 1);
        state.cache.retain(|i, _| (keep_lo..=keep_hi).contains(i));

        // Whatever was queued was queued for a window that has moved.
        state.warm.clear();
        for i in window(index, len) {
            if !state.cache.contains_key(&i) {
                state.warm.push_back(i);
            }
        }
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

/// The frames to hold, ordered by how soon the user is likely to reach them.
/// Forward first, because that is the direction a culling pass runs.
fn window(index: usize, len: usize) -> Vec<usize> {
    let mut out = Vec::with_capacity(AHEAD + BEHIND + 1);
    for d in 0..=AHEAD {
        if index + d < len {
            out.push(index + d);
        }
    }
    for d in 1..=BEHIND {
        if let Some(i) = index.checked_sub(d) {
            out.push(i);
        }
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_runs_forward_first_and_stops_at_the_ends() {
        // Forward before backward, because that is the direction a pass runs.
        assert_eq!(window(10, 100), [10, 11, 12, 13, 14, 15, 9, 8, 7]);
        assert_eq!(window(0, 100), [0, 1, 2, 3, 4, 5]);
        assert_eq!(window(1, 3), [1, 2, 0]);
        assert_eq!(window(0, 1), [0]);
    }

    #[test]
    fn the_window_never_exceeds_the_ring_the_front_end_holds() {
        for index in 0..50 {
            assert!(window(index, 50).len() <= AHEAD + BEHIND + 1);
        }
    }
}
