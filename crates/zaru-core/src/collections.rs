//! Collection names, and the rules they have to survive.
//!
//! A collection is a subfolder of the working folder and nothing else — no
//! database, no catalogue, no hidden state. That makes the name a filename,
//! so the constraints are the filesystem's rather than the app's, and the
//! strictest filesystem in play is Windows'.

/// How many collections the keyboard can reach.
///
/// `M` picks by a single digit, `1` through `9`, because two keystrokes with no
/// typing is the whole point of the shortcut. A tenth collection would have no
/// key, so the limit is real rather than arbitrary.
pub const MAX_COLLECTIONS: usize = 9;

/// Reserved on Windows whatever extension follows them, so `CON` and `CON.raw`
/// are both refused.
const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM0", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
    "COM8", "COM9", "LPT0", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

const ILLEGAL: &[char] = &['/', '\\', ':', '*', '?', '"', '<', '>', '|'];

const MAX_LEN: usize = 64;

/// Checks a name typed into the new-collection field and returns it trimmed.
///
/// Rejecting here, before anything is created, is what keeps the Apply step
/// from failing halfway through with files already moved.
pub fn validate(name: &str, existing: &[String]) -> Result<String, String> {
    let name = name.trim();

    if name.is_empty() {
        return Err("o nome não pode ser vazio".into());
    }
    if name.chars().count() > MAX_LEN {
        return Err(format!("o nome passa de {MAX_LEN} caracteres"));
    }
    if let Some(c) = name.chars().find(|c| ILLEGAL.contains(c)) {
        return Err(format!("o nome não pode conter {c}"));
    }
    if name.chars().any(|c| (c as u32) < 0x20) {
        return Err("o nome não pode conter caracteres de controle".into());
    }
    if name == "." || name == ".." {
        return Err("esse nome é do próprio sistema de arquivos".into());
    }
    // Windows silently drops a trailing dot, so the folder would not have the
    // name the user typed.
    if name.ends_with('.') {
        return Err("o nome não pode terminar em ponto".into());
    }

    let stem = name.split('.').next().unwrap_or(name);
    if RESERVED.iter().any(|r| r.eq_ignore_ascii_case(stem)) {
        return Err(format!("{name} é um nome reservado pelo Windows"));
    }

    // Windows compares filenames without case, so two collections differing
    // only in case would land in the same folder.
    if existing.iter().any(|e| e.eq_ignore_ascii_case(name)) {
        return Err(format!("já existe uma coleção chamada {name}"));
    }

    Ok(name.to_string())
}
