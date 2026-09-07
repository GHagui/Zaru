//! Which key does what.
//!
//! Zaru exists because Photo Mechanic's key map is fixed and did not fit a
//! Colemak-DH split keyboard. Shipping a map that is merely a *different* fixed
//! map would reproduce the original problem for the next person, so the map is
//! data: every action is bound by name, the defaults suit the keyboard this was
//! written for, and anyone else rebinds them.
//!
//! Bindings are `KeyboardEvent.key` values, never `code`. With Colemak-DH
//! active in the operating system the R key reports `KeyS`, because that is its
//! physical QWERTY position; with the layout in the keyboard's own firmware the
//! two agree. `key` is right in both cases.

use serde::{Deserialize, Serialize};

/// Actions that take a single key. The order is the order they are listed in.
pub const ACTIONS: &[(&str, &str)] = &[
    ("prev", "foto anterior"),
    ("next", "próxima foto"),
    ("star1", "1 estrela"),
    ("star2", "2 estrelas"),
    ("star3", "3 estrelas"),
    ("star4", "4 estrelas"),
    ("star5", "5 estrelas"),
    ("label", "etiqueta verde"),
    ("reject", "rejeitar e avançar"),
    ("zoom", "zoom 1:1"),
    ("compare", "comparar"),
    ("filter", "filtrar"),
    ("newCollection", "nova coleção"),
    ("moveTo", "lista de coleções"),
    ("open", "abrir pasta"),
    ("settings", "ajustes"),
    ("help", "teclas"),
];

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Keymap {
    pub prev: String,
    pub next: String,
    pub star1: String,
    pub star2: String,
    pub star3: String,
    pub star4: String,
    pub star5: String,
    pub label: String,
    pub reject: String,
    pub zoom: String,
    pub compare: String,
    pub filter: String,
    pub new_collection: String,
    pub move_to: String,
    pub open: String,
    pub settings: String,
    pub help: String,
    /// One key per collection, in order. How many there are is also the limit
    /// on how many collections can exist: a collection with no key is a
    /// collection the keyboard cannot reach.
    pub collections: Vec<String>,
}

impl Default for Keymap {
    /// Colemak-DH on a 42-key split, which is the keyboard Zaru was written on.
    ///
    /// The ratings sit on the left home row, navigation on the two adjacent
    /// keys of the right bottom row, and the collections take the whole top
    /// row — a split like this has no number row, so digits would cost a layer
    /// for the one action that happens most.
    fn default() -> Self {
        Keymap {
            prev: "k".into(),
            next: "h".into(),
            star1: "a".into(),
            star2: "r".into(),
            star3: "s".into(),
            star4: "t".into(),
            star5: "g".into(),
            label: " ".into(),
            reject: "Backspace".into(),
            zoom: "z".into(),
            compare: "v".into(),
            // `f` belongs to the collections row, so filtering moved one key over.
            filter: "d".into(),
            new_collection: "n".into(),
            move_to: "m".into(),
            open: "o".into(),
            settings: "c".into(),
            help: "?".into(),
            collections: ["q", "w", "f", "p", "b", "j", "l", "u", "y", ";"]
                .iter()
                .map(|k| k.to_string())
                .collect(),
        }
    }
}

impl Keymap {
    /// Every binding, as (action id, key). Collections come last as
    /// `collection1`, `collection2`, and so on.
    pub fn entries(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = ACTIONS
            .iter()
            .map(|(id, _)| ((*id).to_string(), self.get(id).unwrap_or_default()))
            .collect();
        for (i, key) in self.collections.iter().enumerate() {
            out.push((format!("collection{}", i + 1), key.clone()));
        }
        out
    }

    pub fn get(&self, action: &str) -> Option<String> {
        let key = match action {
            "prev" => &self.prev,
            "next" => &self.next,
            "star1" => &self.star1,
            "star2" => &self.star2,
            "star3" => &self.star3,
            "star4" => &self.star4,
            "star5" => &self.star5,
            "label" => &self.label,
            "reject" => &self.reject,
            "zoom" => &self.zoom,
            "compare" => &self.compare,
            "filter" => &self.filter,
            "newCollection" => &self.new_collection,
            "moveTo" => &self.move_to,
            "open" => &self.open,
            "settings" => &self.settings,
            "help" => &self.help,
            other => {
                return self
                    .collection_index(other)
                    .and_then(|i| self.collections.get(i))
                    .cloned()
            }
        };
        Some(key.clone())
    }

    /// Binds `key` to `action`, refusing anything that would leave two actions
    /// on one key. A silent overwrite would take a binding away somewhere the
    /// user is not looking.
    pub fn set(&mut self, action: &str, key: &str) -> Result<(), String> {
        if key.is_empty() {
            return Err("tecla vazia".into());
        }
        if let Some((other, _)) = self
            .entries()
            .iter()
            .find(|(id, bound)| id != action && same_key(bound, key))
        {
            let label = describe(other);
            return Err(format!("{} já está em {label}", show(key)));
        }

        let key = key.to_string();
        match action {
            "prev" => self.prev = key,
            "next" => self.next = key,
            "star1" => self.star1 = key,
            "star2" => self.star2 = key,
            "star3" => self.star3 = key,
            "star4" => self.star4 = key,
            "star5" => self.star5 = key,
            "label" => self.label = key,
            "reject" => self.reject = key,
            "zoom" => self.zoom = key,
            "compare" => self.compare = key,
            "filter" => self.filter = key,
            "newCollection" => self.new_collection = key,
            "moveTo" => self.move_to = key,
            "open" => self.open = key,
            "settings" => self.settings = key,
            "help" => self.help = key,
            other => match self.collection_index(other) {
                Some(i) if i < self.collections.len() => self.collections[i] = key,
                _ => return Err(format!("ação desconhecida: {other}")),
            },
        }
        Ok(())
    }

    fn collection_index(&self, action: &str) -> Option<usize> {
        action
            .strip_prefix("collection")?
            .parse::<usize>()
            .ok()
            .filter(|n| *n >= 1)
            .map(|n| n - 1)
    }
}

/// Keys compare without case, because a letter reaches the handler as `Q` when
/// shift is down and `q` when it is not, and both mean the same binding.
fn same_key(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

fn show(key: &str) -> String {
    match key {
        " " => "espaço".into(),
        other => other.to_string(),
    }
}

fn describe(action: &str) -> String {
    if let Some(rest) = action.strip_prefix("collection") {
        return format!("coleção {rest}");
    }
    ACTIONS
        .iter()
        .find(|(id, _)| *id == action)
        .map(|(_, label)| (*label).to_string())
        .unwrap_or_else(|| action.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_map_binds_every_action_exactly_once() {
        let map = Keymap::default();
        let entries = map.entries();
        assert_eq!(entries.len(), ACTIONS.len() + map.collections.len());

        let mut seen: Vec<String> = Vec::new();
        for (action, key) in &entries {
            assert!(!key.is_empty(), "{action} is unbound");
            assert!(
                !seen.iter().any(|k| same_key(k, key)),
                "{key} is bound twice, second time to {action}"
            );
            seen.push(key.clone());
        }
    }

    #[test]
    fn the_collections_take_the_top_row_of_colemak_dh() {
        // A 42-key split has no number row, so the action that happens most
        // cannot be the one that costs a layer.
        let map = Keymap::default();
        assert_eq!(map.collections, ["q", "w", "f", "p", "b", "j", "l", "u", "y", ";"]);
        // Which is why filtering is not on `f` any more.
        assert_eq!(map.filter, "d");
    }

    #[test]
    fn rebinding_refuses_to_steal_a_key_from_another_action() {
        let mut map = Keymap::default();
        let clash = map.set("zoom", "h").unwrap_err();
        assert!(clash.contains("próxima foto"), "{clash}");
        assert_eq!(map.zoom, "z", "and nothing changed");

        // Case is not a difference: `Q` and `q` are one key.
        assert!(map.set("zoom", "Q").is_err());
    }

    #[test]
    fn a_key_can_be_moved_once_the_old_binding_is_out_of_the_way() {
        let mut map = Keymap::default();
        map.set("filter", "x").unwrap();
        map.set("zoom", "d").unwrap();
        assert_eq!(map.get("zoom").as_deref(), Some("d"));
        assert_eq!(map.get("filter").as_deref(), Some("x"));
    }

    #[test]
    fn a_collection_key_is_bound_by_position() {
        let mut map = Keymap::default();
        map.set("collection1", "1").unwrap();
        assert_eq!(map.collections[0], "1");
        assert_eq!(map.get("collection1").as_deref(), Some("1"));
        assert!(map.set("collection99", "x").is_err());
    }

    #[test]
    fn rebinding_to_the_same_key_is_not_a_conflict_with_itself() {
        let mut map = Keymap::default();
        assert!(map.set("zoom", "z").is_ok());
    }

    #[test]
    fn a_settings_file_from_before_the_keymap_still_loads() {
        let old = r#"{"xmpCompat":"darktable"}"#;
        let parsed: crate::settings::Settings = serde_json::from_str(old).unwrap();
        assert_eq!(parsed.keymap, Keymap::default());
    }
}
