//! Everything Zaru does that is not drawing: which photos are in the folder,
//! what the user marked them with, what gets written where, and keeping the
//! next few previews in memory.
//!
//! None of it depends on Tauri, so it can be tested without a webview — which
//! matters, because the machine this is developed on has no display.

pub mod collections;
pub mod prefetch;
pub mod session;
pub mod settings;

pub use collections::MAX_COLLECTIONS;
pub use prefetch::{Frame, Prefetch};
pub use session::{
    ApplyPlan, ApplyReport, PhotoChange, PhotoView, PlannedMove, Session, SessionView, WriteReport,
};
pub use settings::{Settings, XmpCompat};
