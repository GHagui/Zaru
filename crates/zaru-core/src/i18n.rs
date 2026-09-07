//! Text that has to reach a person, named rather than written.
//!
//! Zaru puts real sentences on screen from Rust: why a collection name was
//! refused, what is blocking an Apply, which action already owns a key. If
//! those stayed as Portuguese strings, a translator could reach every label in
//! the interface and still be stuck with a handful of messages in a language
//! they do not read — which is a translation that looks finished and is not.
//!
//! So Rust names the message and the front end writes it. One file per language
//! then covers the whole app, and adding a language means adding a file.

use std::collections::BTreeMap;
use std::fmt::Display;

use serde::{Deserialize, Serialize};

/// A message identified by key, with whatever the sentence needs filled in.
///
/// The parameters travel separately from the text on purpose: word order moves
/// between languages, and a sentence assembled by concatenation in Rust could
/// not be rearranged by the translator.
#[derive(Clone, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct Message {
    pub key: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, String>,
}

impl Message {
    pub fn new(key: &str) -> Self {
        Message { key: key.to_string(), params: BTreeMap::new() }
    }

    pub fn with(mut self, name: &str, value: impl Display) -> Self {
        self.params.insert(name.to_string(), value.to_string());
        self
    }
}

impl Display for Message {
    /// The key and its parameters, for a log or a test. Never for the screen —
    /// what a person reads is written by the front end from a locale file.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.key)?;
        if !self.params.is_empty() {
            let joined: Vec<String> =
                self.params.iter().map(|(k, v)| format!("{k}={v}")).collect();
            write!(f, " ({})", joined.join(", "))?;
        }
        Ok(())
    }
}

impl From<&str> for Message {
    fn from(key: &str) -> Self {
        Message::new(key)
    }
}

/// The languages that ship with the app, most spoken first.
///
/// A locale dropped into the user's own folder is offered alongside these, so
/// a translator can see their work without a compiler.
pub const BUNDLED: &[(&str, &str)] = &[
    ("en", "English"),
    ("zh-Hans", "简体中文"),
    ("pt-BR", "Português (Brasil)"),
    ("ja", "日本語"),
];

/// The language and, when the tag names one, the writing system.
///
/// Region is deliberately dropped: `pt-PT` and `pt-BR` differ in vocabulary but
/// a reader of one reads the other. Script is deliberately kept: `zh-Hans` and
/// `zh-Hant` are the same language written in characters that somebody who only
/// knows one cannot read, so serving one for the other is not a near miss — it
/// is the wrong text.
fn language_and_script(tag: &str) -> (String, Option<String>) {
    let mut parts = tag.trim().replace('_', "-").to_lowercase();
    parts.retain(|c| c != ' ');
    let mut pieces = parts.split('-');
    let language = pieces.next().unwrap_or_default().to_string();
    // A script subtag is the four-letter one, which is what tells `Hans` from a
    // region like `BR` or a variant.
    let script = pieces.find(|p| p.len() == 4 && p.chars().all(|c| c.is_ascii_alphabetic()));
    (language, script.map(str::to_string))
}

/// Picks the closest shipped language to what the system asks for.
///
/// An exact tag wins; failing that, the same language in the same script, so
/// `pt-PT` finds `pt-BR` and `ja-JP` finds `ja` while `zh-Hant` finds nothing
/// and falls through to English.
pub fn best_match(requested: &str, available: &[String]) -> Option<String> {
    if requested.trim().is_empty() {
        return None;
    }
    let lower = requested.trim().replace('_', "-").to_lowercase();

    if let Some(exact) = available.iter().find(|a| a.to_lowercase() == lower) {
        return Some(exact.clone());
    }

    let (language, script) = language_and_script(requested);
    if language.is_empty() {
        return None;
    }
    available.iter().find(|candidate| {
        let (their_language, their_script) = language_and_script(candidate);
        their_language == language
            // Absent on either side means "unspecified", which is compatible;
            // two different scripts never are.
            && match (&script, &their_script) {
                (Some(a), Some(b)) => a == b,
                _ => true,
            }
    }).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn available() -> Vec<String> {
        BUNDLED.iter().map(|(tag, _)| tag.to_string()).collect()
    }

    #[test]
    fn a_message_carries_its_parts_and_not_a_sentence() {
        let m = Message::new("collection.exists").with("name", "porsche");
        assert_eq!(m.key, "collection.exists");
        assert_eq!(m.params.get("name").map(String::as_str), Some("porsche"));
        // Nothing here is prose: word order belongs to whoever translates it.
        assert!(!m.key.contains(' '));
    }

    #[test]
    fn a_region_finds_its_language() {
        assert_eq!(best_match("ja-JP", &available()).as_deref(), Some("ja"));
        assert_eq!(best_match("pt-PT", &available()).as_deref(), Some("pt-BR"));
        assert_eq!(best_match("en-GB", &available()).as_deref(), Some("en"));
        assert_eq!(best_match("zh-Hans-CN", &available()).as_deref(), Some("zh-Hans"));
        // Underscores are what some systems report.
        assert_eq!(best_match("pt_BR", &available()).as_deref(), Some("pt-BR"));
    }

    #[test]
    fn a_language_we_do_not_have_matches_nothing() {
        // The caller falls back to English rather than guessing at a neighbour.
        assert_eq!(best_match("ru-RU", &available()), None);
        assert_eq!(best_match("", &available()), None);
        // `zh-Hant` is a different written language from `zh-Hans` and must not
        // silently borrow it.
        assert_eq!(best_match("zh-Hant", &available()), None);
    }

    #[test]
    fn a_locale_the_user_dropped_in_is_matchable_too() {
        let mut theirs = available();
        theirs.push("it-IT".into());
        assert_eq!(best_match("it", &theirs).as_deref(), Some("it-IT"));
    }
}
