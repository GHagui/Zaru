//! Everything Zaru does that is not drawing: which photos are in the folder,
//! what the user marked them with, what gets written where, and keeping the
//! next few previews in memory.
//!
//! None of it depends on Tauri, so it can be tested without a webview — which
//! matters, because the machine this is developed on has no display.

pub mod collections;
pub mod keymap;
pub mod prefetch;
pub mod recovery;
pub mod session;
pub mod thumbs;
pub mod settings;

pub use keymap::Keymap;
pub use zaru_cr3::Exif;
pub use prefetch::{Frame, Prefetch};
pub use recovery::{Recovery, RecoveryOffer};
pub use thumbs::{Source as ThumbSource, Thumbs};
pub use session::{
    ApplyOperation, BatchEdit,
    ApplyPlan, ApplyReport, PhotoChange, PhotoView, PlannedMove, Session, SessionView, WriteReport,
};
pub use settings::{Settings, XmpCompat};
