//! Collection names, and the rules they have to survive.
//!
//! A collection is a subfolder of the working folder and nothing else — no
//! database, no catalogue, no hidden state. That makes the name a filename,
//! so the constraints are the filesystem's rather than the app's, and the
//! strictest filesystem in play is Windows'.

/// Reserved on Windows whatever extension follows them, so `CON` and `CON.raw`
/// are both refused.
const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM0", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
    "COM8", "COM9", "LPT0", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

const ILLEGAL: &[char] = &['/', '\\', ':', '*', '?', '"', '<', '>', '|'];

const MAX_LEN: usize = 64;

use crate::i18n::Message;

/// Checks a name typed into the new-collection field and returns it trimmed.
///
/// Rejecting here, before anything is created, is what keeps the Apply step
/// from failing halfway through with files already moved. Every refusal is
/// named rather than written, so the reason reaches the user in their language.
pub fn validate(name: &str, existing: &[String]) -> Result<String, Message> {
    let name = name.trim();

    if name.is_empty() {
        return Err(Message::new("collection.name.empty"));
    }
    if name.chars().count() > MAX_LEN {
        return Err(Message::new("collection.name.tooLong").with("max", MAX_LEN));
    }
    if let Some(c) = name.chars().find(|c| ILLEGAL.contains(c)) {
        return Err(Message::new("collection.name.illegalChar").with("char", c));
    }
    if name.chars().any(|c| (c as u32) < 0x20) {
        return Err(Message::new("collection.name.controlChar"));
    }
    if name == "." || name == ".." {
        return Err(Message::new("collection.name.filesystem"));
    }
    // Windows silently drops a trailing dot, so the folder would not have the
    // name the user typed.
    if name.ends_with('.') {
        return Err(Message::new("collection.name.trailingDot"));
    }

    let stem = name.split('.').next().unwrap_or(name);
    if RESERVED.iter().any(|r| r.eq_ignore_ascii_case(stem)) {
        return Err(Message::new("collection.name.reserved").with("name", name));
    }

    // Windows compares filenames without case, so two collections differing
    // only in case would land in the same folder.
    if existing.iter().any(|e| e.eq_ignore_ascii_case(name)) {
        return Err(Message::new("collection.name.exists").with("name", name));
    }

    Ok(name.to_string())
}
