"use strict";

// Every word a person reads comes from a locale file, including the messages
// Rust sends up: those arrive as a key and its parts, never as a sentence, so
// whoever translates the interface translates all of it.
//
// A translator needs no toolchain. Copy `ui/locales/en.json`, translate the
// values, drop the file in the app's `locales` folder, and it appears in the
// language list on the next start.

const FALLBACK = "en";

const i18n = {
  tag: FALLBACK,
  strings: {},
  fallback: {},
  /// Keys asked for and not found, so a partial translation is visible rather
  /// than silently English.
  missing: new Set(),
};

/// Fills `{name}` holes from the parameters a message carries.
///
/// The holes are named rather than positional because word order moves between
/// languages: "porsche já existe" and "porsche already exists" put the name in
/// different places, and a positional slot could not follow it.
function fill(template, params) {
  if (!params) return template;
  return template.replace(/\{(\w+)\}/g, (whole, name) =>
    Object.prototype.hasOwnProperty.call(params, name) ? String(params[name]) : whole,
  );
}

/// The text for a key, in the chosen language, falling back to English and then
/// to the key itself.
///
/// Showing the key is deliberate: a blank space hides the gap, while
/// `collection.name.exists` on screen tells a translator exactly what to add.
function t(key, params) {
  const found = i18n.strings[key] ?? i18n.fallback[key];
  if (found === undefined) {
    i18n.missing.add(key);
    return key;
  }
  return fill(found, params);
}

/// Text for a count, choosing between `<key>.one` and `<key>.other`.
///
/// Two forms rather than a full plural system: the app counts photos, files and
/// collections, and no language it ships in needs more than that. Chinese and
/// Japanese do not inflect at all, and their files simply carry the same
/// sentence in both entries.
function tc(key, count, params = {}) {
  const form = count === 1 ? `${key}.one` : `${key}.other`;
  return t(form, { count, ...params });
}

/// Renders a message the way Rust sent it: a key and its parts.
function tm(message) {
  if (!message) return "";
  if (typeof message === "string") return t(message);
  return t(message.key, message.params);
}

/// Builds a node tree from a translated string, turning `[[x]]` into a key cap.
///
/// Sentences with a key in the middle have to stay one string, or a translator
/// cannot move the key to where their language puts it. The markers are
/// expanded into real elements here, so nothing a locale file contains is ever
/// treated as HTML.
function rich(text) {
  const out = document.createDocumentFragment();
  for (const piece of text.split(/(\[\[[^\]]*\]\])/)) {
    if (!piece) continue;
    if (piece.startsWith("[[") && piece.endsWith("]]")) {
      const cap = document.createElement("kbd");
      cap.textContent = piece.slice(2, -2);
      out.append(cap);
    } else {
      out.append(document.createTextNode(piece));
    }
  }
  return out;
}

/// Applies the current language to the document.
///
/// `data-i18n` replaces an element's text; `data-i18n-attr` carries
/// `attribute:key` pairs for the things that speak without being visible, like
/// `aria-label` and `placeholder`.
function translateDocument(root = document) {
  for (const node of root.querySelectorAll("[data-i18n]")) {
    node.textContent = t(node.dataset.i18n);
  }
  for (const node of root.querySelectorAll("[data-i18n-rich]")) {
    node.replaceChildren(rich(t(node.dataset.i18nRich)));
  }
  for (const node of root.querySelectorAll("[data-i18n-attr]")) {
    for (const pair of node.dataset.i18nAttr.split(",")) {
      const [attribute, key] = pair.split(":").map((s) => s.trim());
      if (attribute && key) node.setAttribute(attribute, t(key));
    }
  }
  document.documentElement.lang = i18n.tag;
}

/// Loads one locale file that ships with the app.
async function loadBundled(tag) {
  const response = await fetch(`locales/${tag}.json`);
  if (!response.ok) throw new Error(`sem locale ${tag}`);
  return response.json();
}

/// Chooses and applies a language.
///
/// `extra` holds locale files the user dropped into their own folder, which the
/// Rust side read for us; they win over the bundled ones of the same name so a
/// translator can correct a shipped language without waiting for a release.
async function useLanguage(tag, extra = {}) {
  i18n.fallback = await loadBundled(FALLBACK).catch(() => ({}));
  i18n.strings =
    extra[tag] ?? (tag === FALLBACK ? i18n.fallback : await loadBundled(tag).catch(() => ({})));
  i18n.tag = tag;
  i18n.missing.clear();
  translateDocument();
  return i18n.strings;
}

// `app.js` and `grid.js` are classic scripts sharing one scope, so the helpers
// are published once here rather than being destructured in each of them —
// declaring `t` twice makes the second file fail to parse.
window.zaruI18n = { t, tc, tm, rich, useLanguage, translateDocument, state: i18n, FALLBACK };
window.t = t;
window.tc = tc;
window.tm = tm;
