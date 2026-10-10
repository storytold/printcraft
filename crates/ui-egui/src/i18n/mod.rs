//! UI localisation. Strings in code stay English and are the default lookup keys; a per-language
//! catalog (`*.tsv`, see `ja.tsv` for the format) maps them to display text at render time. Command
//! ids, document text, file names, the control channel, the CLI and MCP never see translated text,
//! so agents and scripts are unaffected. A string without a translation is shown in English, so
//! coverage can grow incrementally.
//!
//! The system is the one PhotoCraft uses (`photocraft/crates/ui-egui/src/i18n`), so catalogs and
//! habits carry over between the two apps.
//!
//! # Adding a language
//! 1. Add `xx.tsv` next to `ja.tsv` (copy its header; translate from the *meaning* of the English
//!    text, clean-room, see `ja.tsv`).
//! 2. Add one row to [`LANGUAGES`] (code, native name, catalog, plural rule).
//!
//! That is all: the Preferences dropdown, the system-locale match and the catalog tests (parse,
//! placeholders, plural forms) pick it up from the registry.
//!
//! # Looking strings up
//! - [`tl!`](crate::tl) / [`t`]: a plain string in the current language. [`tr`]: in a given one.
//! - [`tr_ctx`] (`tl_ctx!` in UI code): when one English word needs different translations.
//! - [`tr_id`]: a command-id keyed string with the English label as fallback (menu items), so a
//!   translation survives rewording of the English text and can differ per command.
//! - [`trn`]: plural-aware (`{n}` is filled in). [`fmt`]: fill `{name}` placeholders after [`tr`];
//!   translators may reorder placeholders freely.

mod catalog;

use std::cell::Cell;
use std::sync::OnceLock;

use catalog::Catalog;

/// The `language` preference value that follows the system locale.
pub const AUTO: &str = "auto";

/// One supported UI language.
pub struct LangInfo {
    /// BCP 47 code, lowercase (`ja`, `zh-hans`, `pt-br`). Also the `language` preference value.
    pub code: &'static str,
    /// The language's name in itself, shown in the Preferences dropdown.
    pub name: &'static str,
    /// Catalog file contents (empty for the built-in English).
    pub source: &'static str,
    /// Plural form index for a count (English: 0 = one, 1 = other; Japanese: always 0). A catalog's
    /// `@plural` entries list one form per index.
    pub plural: fn(u64) -> usize,
    /// The language is written right to left. egui lays text out left to right, so the catalog's
    /// translations are put into display order when it loads ([`crate::bidi::display_rtl`]).
    pub rtl: bool,
    catalog: OnceLock<Catalog>,
}

fn plural_one_other(n: u64) -> usize {
    usize::from(n != 1)
}

fn plural_none(_: u64) -> usize {
    0
}

/// Czech: 1 → one, 2–4 → few, everything else (0, 5+) → other.
fn plural_cs(n: u64) -> usize {
    match n {
        1 => 0,
        2..=4 => 1,
        _ => 2,
    }
}

/// Portuguese: 0 and 1 take the singular, everything else the plural.
fn plural_pt(n: u64) -> usize {
    usize::from(n > 1)
}

/// French (CLDR `fr`): 0 and 1 take the singular, everything else the plural.
fn plural_fr(n: u64) -> usize {
    usize::from(n > 1)
}

/// Russian: 1 (but not 11) is `one`, 2–4 (but not 12–14) `few`, everything else `many`.
fn plural_russian(n: u64) -> usize {
    match n % 100 {
        11..=14 => 2,
        _ => match n % 10 {
            1 => 0,
            2..=4 => 1,
            _ => 2,
        },
    }
}

/// Ukrainian (CLDR `uk`, integer counts): one (1, 21), few (2–4, 22–24), many (0, 5–19).
/// Counts ending in 11–14 always take many, regardless of the last digit.
fn plural_ukrainian(n: u64) -> usize {
    match n % 100 {
        11..=14 => 2,
        _ => match n % 10 {
            1 => 0,
            2..=4 => 1,
            _ => 2,
        },
    }
}

/// Arabic (CLDR `ar`, integer counts): zero (0), one (1), two (2), few (3–10, 103–110),
/// many (11–99, 111–199), other (100–102, 200–202).
fn plural_arabic(n: u64) -> usize {
    match (n, n % 100) {
        (0, _) => 0,
        (1, _) => 1,
        (2, _) => 2,
        (_, 3..=10) => 3,
        (_, 11..=99) => 4,
        _ => 5,
    }
}

/// Polish integer counts: 1 is one; endings 2–4 except 12–14 are few; the rest are many.
fn plural_polish(n: u64) -> usize {
    match (n, n % 100, n % 10) {
        (1, _, _) => 0,
        (_, 12..=14, _) => 2,
        (_, _, 2..=4) => 1,
        _ => 2,
    }
}

/// The registry. English first: it is the fallback and the source language.
pub static LANGUAGES: [LangInfo; 17] = [
    LangInfo { code: "en", name: "English", source: "", plural: plural_one_other, rtl: false, catalog: OnceLock::new() },
    LangInfo { code: "ja", name: "日本語", source: include_str!("ja.tsv"), plural: plural_none, rtl: false, catalog: OnceLock::new() },
    // Simplified Chinese; `zh`, `zh-CN`, `zh-SG` and `zh-Hans-*` locales resolve here (see `candidates`).
    LangInfo {
        code: "zh-hans", name: "简体中文", source: include_str!("zh-hans.tsv"), plural: plural_none, rtl: false, catalog: OnceLock::new()
    },
    // Traditional Chinese in the vocabulary used in Taiwan; `zh-TW`, `zh-HK`, `zh-MO` and `zh-Hant-*`
    // locales all resolve here (see `candidates`).
    LangInfo {
        code: "zh-hant", name: "繁體中文", source: include_str!("zh-hant.tsv"), plural: plural_none, rtl: false, catalog: OnceLock::new()
    },
    LangInfo { code: "cs", name: "Čeština", source: include_str!("cs.tsv"), plural: plural_cs, rtl: false, catalog: OnceLock::new() },
    // Brazilian Portuguese; `pt`, `pt-BR` and `pt-PT` locales all resolve here (see `candidates`).
    LangInfo {
        code: "pt-br",
        name: "Português (Brasil)",
        source: include_str!("pt-br.tsv"),
        plural: plural_pt,
        rtl: false,
        catalog: OnceLock::new(),
    },
    // German (informal "du"); every `de-*` locale (`de-DE`, `de-AT`, `de-CH` ...) resolves here.
    LangInfo { code: "de", name: "Deutsch", source: include_str!("de.tsv"), plural: plural_one_other, rtl: false, catalog: OnceLock::new() },
    // Spanish (European vocabulary); every `es-*` locale (`es-ES`, `es-MX`, `es-419` ...) resolves here.
    LangInfo { code: "es", name: "Español", source: include_str!("es.tsv"), plural: plural_one_other, rtl: false, catalog: OnceLock::new() },
    // French; every `fr-*` locale (`fr-FR`, `fr-CA`, `fr-BE` ...) resolves here.
    LangInfo { code: "fr", name: "Français", source: include_str!("fr.tsv"), plural: plural_fr, rtl: false, catalog: OnceLock::new() },
    // Russian; every `ru-*` locale (`ru-RU`, `ru-BY`, `ru-KZ` ...) resolves here.
    LangInfo { code: "ru", name: "Русский", source: include_str!("ru.tsv"), plural: plural_russian, rtl: false, catalog: OnceLock::new() },
    // Bulgarian; every `bg-*` locale (`bg-BG`) resolves here.
    LangInfo {
        code: "bg", name: "Български", source: include_str!("bg.tsv"), plural: plural_one_other, rtl: false, catalog: OnceLock::new()
    },
    // Telugu; every `te-*` locale (`te-IN`) resolves here.
    LangInfo {
        code: "te", name: "తెలుగు", source: include_str!("te.tsv"), plural: plural_one_other, rtl: false, catalog: OnceLock::new()
    },
    // Hungarian; every `hu-*` locale (`hu-HU`) resolves here.
    LangInfo { code: "hu", name: "Magyar", source: include_str!("hu.tsv"), plural: plural_one_other, rtl: false, catalog: OnceLock::new() },
    // Ukrainian; every `uk-*` locale (`uk-UA`, `uk_UA.UTF-8`) resolves here.
    LangInfo {
        code: "uk", name: "Українська", source: include_str!("uk.tsv"), plural: plural_ukrainian, rtl: false, catalog: OnceLock::new()
    },
    // Italian; every `it-*` locale (`it-IT`) resolves here.
    LangInfo { code: "it", name: "Italiano", source: include_str!("it.tsv"), plural: plural_one_other, rtl: false, catalog: OnceLock::new() },
    // Arabic (Modern Standard, Western digits); every `ar-*` locale (`ar-MA`, `ar-EG`, `ar-SA` ...)
    // resolves here. Right to left: see `LangInfo::rtl`.
    LangInfo { code: "ar", name: "العربية", source: include_str!("ar.tsv"), plural: plural_arabic, rtl: true, catalog: OnceLock::new() },
    // Polish; every `pl-*` locale (`pl-pl`, `pl_pl.UTF-8`) resolves here.
    LangInfo { code: "pl", name: "Polski", source: include_str!("pl.tsv"), plural: plural_polish, rtl: false, catalog: OnceLock::new() },
];

impl LangInfo {
    /// How many plural forms the language's `@plural` entries list.
    fn plural_forms(&self) -> usize {
        (0..=1000).map(self.plural).max().unwrap_or(0).saturating_add(1)
    }

    fn catalog(&self) -> &Catalog {
        self.catalog.get_or_init(|| {
            let (mut catalog, errors) = Catalog::parse(self.source, self.plural_forms());
            for error in errors {
                log::warn!("{} interface catalog: {error}; that entry shows in English", self.code);
            }
            if self.rtl {
                catalog.map_translations(crate::bidi::display_rtl);
            }
            catalog
        })
    }
}

/// A language the UI can be shown in (a handle into [`LANGUAGES`]).
#[derive(Clone, Copy)]
pub struct Lang(&'static LangInfo);

impl std::fmt::Debug for Lang {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Lang({})", self.0.code)
    }
}

impl PartialEq for Lang {
    fn eq(&self, other: &Self) -> bool {
        self.0.code == other.0.code
    }
}

impl Eq for Lang {}

impl Lang {
    pub const EN: Lang = Lang(&LANGUAGES[0]);

    pub fn code(self) -> &'static str {
        self.0.code
    }

    /// A language by its exact code.
    pub fn from_code(code: &str) -> Option<Lang> {
        LANGUAGES.iter().find(|l| l.code.eq_ignore_ascii_case(code)).map(Lang)
    }

    /// Resolve the `language` preference: a language code, or [`AUTO`] (and anything unknown,
    /// e.g. a code from a newer version) to follow the system locale.
    pub fn from_pref(pref: &str) -> Lang {
        Lang::from_code(pref).unwrap_or_else(system_lang)
    }

    /// Every registered language.
    pub fn all() -> impl Iterator<Item = Lang> {
        LANGUAGES.iter().map(Lang)
    }

    pub fn name(self) -> &'static str {
        self.0.name
    }

    /// Whether the language is written right to left. Its translations come back from the
    /// lookups in display order (see [`LangInfo::rtl`]).
    pub fn rtl(self) -> bool {
        self.0.rtl
    }

    fn catalog(self) -> &'static Catalog {
        self.0.catalog()
    }
}

/// The canonical `language` preference for user input: [`AUTO`] or a registered code (any case).
/// `None` for anything else, so callers can keep the current setting.
pub fn normalize_pref(pref: &str) -> Option<&'static str> {
    if pref.eq_ignore_ascii_case(AUTO) {
        return Some(AUTO);
    }
    Lang::from_code(pref).map(Lang::code)
}

/// Candidate language codes for a locale tag, most specific first: `zh_TW.UTF-8` →
/// `zh-tw`, `zh-hant`, `zh`.
fn candidates(tag: &str) -> Vec<String> {
    let base = tag.split(['.', '@']).next().unwrap_or("").replace('_', "-").to_ascii_lowercase();
    let parts: Vec<&str> = base.split('-').filter(|p| !p.is_empty()).collect();
    let Some(&primary) = parts.first() else { return Vec::new() };
    let mut out = Vec::new();
    for n in (1..=parts.len()).rev() {
        out.push(parts.get(..n).unwrap_or_default().join("-"));
    }
    if primary == "zh" && !parts.iter().any(|p| matches!(*p, "hans" | "hant")) {
        // Chinese by region when no script is given.
        let script = if parts.iter().any(|p| matches!(*p, "tw" | "hk" | "mo")) { "zh-hant" } else { "zh-hans" };
        out.insert(out.len().saturating_sub(1), script.to_string());
    }
    if primary == "pt" && !out.iter().any(|c| c == "pt-br") {
        // The only Portuguese catalog is Brazilian; other regions use it rather than English.
        out.insert(out.len().saturating_sub(1), "pt-br".to_string());
    }
    out
}

/// The registered language for a locale tag such as `ja_JP.UTF-8`, `ja-JP`; `None` if the
/// language isn't supported. `C`/`POSIX` mean English.
pub fn lang_from_tag(tag: &str) -> Option<Lang> {
    let cands = candidates(tag);
    if matches!(cands.first().map(String::as_str), Some("c" | "posix")) {
        return Some(Lang::EN);
    }
    cands.iter().find_map(|c| Lang::from_code(c))
}

/// The system language (cached). English when it can't be determined.
pub fn system_lang() -> Lang {
    // Tests drive the UI by its English labels whatever the developer's locale is. This covers
    // the unit tests; the integration tests link the library as it ships, where `cfg!(test)` is
    // off, so each of their harnesses pins the `language` option to `en` instead.
    if cfg!(test) {
        return Lang::EN;
    }
    static SYSTEM: OnceLock<Lang> = OnceLock::new();
    *SYSTEM.get_or_init(detect_system_lang)
}

#[cfg(not(target_arch = "wasm32"))]
fn detect_system_lang() -> Lang {
    for var in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Some(l) = std::env::var(var).ok().and_then(|v| language_override(&v)) {
            return l;
        }
    }
    // Apps started from the Finder don't inherit LANG: use the macOS preferred-languages list.
    // The absolute path keeps a `defaults` earlier on PATH from running; any failure means English.
    #[cfg(target_os = "macos")]
    if let Ok(out) = std::process::Command::new("/usr/bin/defaults").args(["read", "-g", "AppleLanguages"]).output()
        && out.status.success()
        && let Some(l) = first_supported(&String::from_utf8_lossy(&out.stdout))
    {
        return l;
    }
    #[cfg(target_os = "windows")]
    if let Some(l) = windows_ui_language().as_deref().and_then(first_supported) {
        return l;
    }
    Lang::EN
}

/// A named locale can override the display language. On Windows, the neutral
/// C/POSIX locale inherited from a launcher says nothing about the user's UI language.
#[cfg(not(target_arch = "wasm32"))]
fn language_override(tag: &str) -> Option<Lang> {
    let tag = tag.trim();
    if cfg!(target_os = "windows") && matches!(candidates(tag).first().map(String::as_str), Some("c" | "posix")) {
        return None;
    }
    lang_from_tag(tag)
}

/// The Windows display languages in preference order, one tag per line, queried once when Auto
/// first resolves. `sys-locale` asks Windows directly (`GetUserPreferredUILanguages`), so no
/// process is started.
#[cfg(target_os = "windows")]
fn windows_ui_language() -> Option<String> {
    let tags: Vec<String> = sys_locale::get_locales().collect();
    (!tags.is_empty()).then(|| tags.join("\n"))
}

/// The first supported language in a macOS `defaults read` list or Windows UI-culture output.
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
fn first_supported(list: &str) -> Option<Lang> {
    list.split(['(', ')', ',', '"', '\n']).map(str::trim).filter(|s| !s.is_empty()).find_map(lang_from_tag)
}

#[cfg(target_arch = "wasm32")]
fn detect_system_lang() -> Lang {
    Lang::EN
}

thread_local! {
    // Per thread, as in PhotoCraft: independent app and test threads (kittest harnesses run in
    // parallel) must not change each other's drawing language.
    static CURRENT: Cell<Lang> = const { Cell::new(Lang::EN) };
}

/// Set the UI language for drawing (the shell calls this once per frame from the preference), so
/// widgets can translate without every call site carrying a language around.
pub fn set_current(lang: Lang) {
    CURRENT.set(lang);
}

/// The language the UI is drawn in.
/// `label` as it reads inside a sentence ("add a text box"): lowercased, except in languages
/// that capitalise nouns (German), where lowercasing would misspell it.
pub fn in_sentence(label: &str) -> std::borrow::Cow<'_, str> {
    if current().code() == "de" { std::borrow::Cow::Borrowed(label) } else { std::borrow::Cow::Owned(label.to_lowercase()) }
}

pub fn current() -> Lang {
    CURRENT.get()
}

/// Does `lang` have a catalog entry for this plain string? (English never does: it is the source.)
pub fn has(lang: Lang, s: &str) -> bool {
    lang.catalog().plain(s).is_some()
}

/// Translate an English UI string into the current language ([`tr`] with [`current`]).
pub fn t(s: &str) -> &str {
    tr(current(), s)
}

/// The name of a keyboard key, distinct from a command such as Delete. Missing key-name
/// translations use the English key name, never a plain command translation.
pub fn key_name(s: &str) -> &str {
    current().catalog().contextual("key", s).unwrap_or(s)
}

/// Translate an English UI string; unknown strings come back unchanged.
pub fn tr(lang: Lang, s: &str) -> &str {
    lang.catalog().plain(s).unwrap_or(s)
}

/// Like [`tr`], for an English string that needs a disambiguating `context`.
pub fn tr_ctx<'a>(lang: Lang, context: &str, s: &'a str) -> &'a str {
    lang.catalog().contextual(context, s).unwrap_or_else(|| tr(lang, s))
}

/// A string keyed by its command id, falling back to the translation of the English `label`.
pub fn tr_id<'a>(lang: Lang, id: &str, label: &'a str) -> &'a str {
    lang.catalog().id(id).unwrap_or_else(|| tr(lang, label))
}

/// Fill `{name}` placeholders in one pass. Unknown placeholders are left as written, and inserted
/// values are never read as templates again, so a file name containing `{n}` stays intact.
pub fn fmt(template: &str, args: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some((before, after_open)) = rest.split_once('{') {
        out.push_str(before);
        let Some((name, after_close)) = after_open.split_once('}') else {
            out.push('{');
            out.push_str(after_open);
            return out;
        };
        match args.iter().find(|(key, _)| *key == name) {
            Some((_, value)) => out.push_str(value),
            None => {
                out.push('{');
                out.push_str(name);
                out.push('}');
            }
        }
        rest = after_close;
    }
    out.push_str(rest);
    out
}

/// A generated history label (the action part of "Undo …") in the current language. Captured
/// values (file and field names) are inserted as they are; unknown labels are looked up whole.
pub fn action_label(text: &str) -> String {
    for (prefix, template) in [
        ("Insert pages from ", "Insert pages from {name}"),
        ("Fill in ", "Fill in {name}"),
        ("Set the image of ", "Set the image of {name}"),
        ("Edit script of ", "Edit script of {name}"),
        ("Import ", "Import {name}"),
    ] {
        if let Some(value) = text.strip_prefix(prefix) {
            return fmt(t(template), &[("name", value)]);
        }
    }
    if let Some(key) = text.strip_prefix("Change ")
        && ["Title", "Author", "Subject", "Keywords", "Creator", "Producer"].contains(&key)
    {
        return fmt(t("Change {key}"), &[("key", t(key))]);
    }
    t(text).to_owned()
}

/// A command label such as "Undo Insert pages from a.pdf" in the current language. A catalog
/// whose word order doesn't put the verb first translates "Undo {action}" and "Redo {action}"
/// as whole phrases; otherwise the translated verb goes before the action.
pub fn command_label(text: &str) -> String {
    for (prefix, template) in [("Undo", "Undo {action}"), ("Redo", "Redo {action}")] {
        if let Some(action) = text.strip_prefix(prefix).and_then(|tail| tail.strip_prefix(' ')) {
            let action = action_label(action);
            if has(current(), template) {
                return fmt(t(template), &[("action", &action)]);
            }
            return format!("{} {action}", t(prefix));
        }
    }
    t(text).to_owned()
}

/// A registered command's menu label: its `@id` catalog entry if there is one, otherwise
/// [`command_label`] of the current English label.
pub fn menu_label(id: &str, label: &str) -> String {
    match current().catalog().id(id) {
        Some(translated) => translated.to_owned(),
        None => command_label(label),
    }
}

/// A plural-aware message: `one`/`other` are the English forms (with `{n}` where the count goes).
pub fn trn(lang: Lang, n: u64, one: &str, other: &str) -> String {
    let idx = (lang.0.plural)(n);
    let text = lang.catalog().plural(one, other, idx).unwrap_or(if n == 1 { one } else { other });
    fmt(text, &[("n", &n.to_string())])
}

#[cfg(test)]
mod tests {
    use super::catalog::{Entry, parse_entries, placeholders};
    use super::*;

    const JA: fn() -> Lang = || Lang::from_code("ja").expect("ja registered");
    // These physical key-cap names remain English in several keyboard layouts. Only their
    // dedicated key context may use them unchanged; ordinary command labels still translate.
    const KEY_CAP_NAMES: &[&str] = &["Ctrl", "Shift", "Delete", "Esc", "Home", "End"];

    #[test]
    fn keyboard_key_names_have_a_separate_context_and_english_fallback() {
        for lang in Lang::all().filter(|l| *l != Lang::EN) {
            for key in ["Ctrl", "Shift", "Delete", "Esc", "Home", "End", "Space"] {
                assert!(lang.catalog().contextual("key", key).is_some(), "{}: {key}", lang.code());
            }
        }
        let previous = current();
        set_current(Lang::from_code("de").unwrap());
        assert_eq!(key_name("Delete"), "Entf");
        assert_eq!(t("Delete"), "Löschen");
        assert_eq!(key_name("Open"), "Open", "a missing key name must not borrow a command translation");
        set_current(Lang::EN);
        assert_eq!(key_name("Delete"), "Delete");
        set_current(previous);
    }

    #[test]
    fn tags_map_to_languages() {
        assert_eq!(lang_from_tag("ja_JP.UTF-8"), Some(JA()));
        assert_eq!(lang_from_tag("ja-JP"), Some(JA()));
        assert_eq!(lang_from_tag("en_US.UTF-8"), Some(Lang::EN));
        assert_eq!(lang_from_tag("C"), Some(Lang::EN));
        assert_eq!(lang_from_tag("POSIX"), Some(Lang::EN));
        assert_eq!(lang_from_tag("fr_FR"), Lang::from_code("fr"));
        assert_eq!(lang_from_tag("fr_CA.UTF-8"), Lang::from_code("fr"));
        assert_eq!(lang_from_tag("fr-BE"), Lang::from_code("fr"));
        assert_eq!(lang_from_tag("zh-TW"), Lang::from_code("zh-hant"));
        assert_eq!(lang_from_tag("zh_CN.UTF-8"), Lang::from_code("zh-hans"));
        assert_eq!(lang_from_tag("zh"), Lang::from_code("zh-hans"));
        assert_eq!(lang_from_tag("zh_HK.UTF-8"), Lang::from_code("zh-hant"));
        assert_eq!(lang_from_tag("zh-Hant-MO"), Lang::from_code("zh-hant"));
        assert_eq!(lang_from_tag(""), None);
        assert_eq!(lang_from_tag("_"), None);
    }

    #[test]
    fn candidates_walk_from_specific_to_general() {
        assert_eq!(candidates("pt_BR.UTF-8"), ["pt-br", "pt"]);
        assert_eq!(candidates("pt_PT.UTF-8"), ["pt-pt", "pt-br", "pt"]);
        assert_eq!(candidates("pt"), ["pt-br", "pt"]);
        assert_eq!(candidates("zh_TW"), ["zh-tw", "zh-hant", "zh"]);
        assert_eq!(candidates("zh-CN"), ["zh-cn", "zh-hans", "zh"]);
        assert_eq!(candidates("zh-Hant-HK"), ["zh-hant-hk", "zh-hant", "zh"]);
    }

    #[test]
    fn macos_language_list_is_parsed() {
        assert_eq!(first_supported("(\n    \"ja-JP\",\n    \"en-US\"\n)\n"), Some(JA()));
        assert_eq!(first_supported("(\n    \"fr-FR\",\n    \"en-US\"\n)\n"), Lang::from_code("fr"));
        assert_eq!(first_supported("("), None);
    }

    #[test]
    fn windows_display_language_resolves_chinese_scripts() {
        for tag in ["zh-CN", "zh-SG", "zh-Hans", "zh-Hans-CN"] {
            assert_eq!(first_supported(&format!("{tag}\r\n")), Lang::from_code("zh-hans"));
        }
        for tag in ["zh-TW", "zh-HK", "zh-MO", "zh-Hant", "zh-Hant-HK"] {
            assert_eq!(first_supported(&format!("{tag}\r\n")), Lang::from_code("zh-hant"));
        }
        assert_eq!(first_supported("en-US\r\n"), Some(Lang::EN));
        assert_eq!(first_supported("fr-FR\r\n"), Lang::from_code("fr"));
        assert_eq!(first_supported("\r\n"), None);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn neutral_locale_does_not_override_display_language() {
        let fr = Lang::from_code("fr").unwrap();
        for tag in ["C", "C.UTF-8", "POSIX", "posix.UTF-8", "", "  C.UTF-8  ", "unsupported"] {
            assert_eq!(language_override(tag), None, "{tag}");
            assert_eq!(language_override(tag).or_else(|| first_supported("unsupported-XY\nfr-FR\nen-US")), Some(fr));
        }
        assert_eq!(["C.UTF-8", "POSIX", "fr_FR.UTF-8"].into_iter().find_map(language_override), Some(fr));
        assert_eq!(language_override("fr_FR.UTF-8"), Some(fr));
        assert_eq!(language_override("en_US.UTF-8"), Some(Lang::EN));
        assert_eq!(language_override("ja_JP.UTF-8"), Some(JA()));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_display_language_query_returns_a_locale_tag() {
        // One tag per line, as `first_supported` reads them: a machine with several display
        // languages reports them all, so each line is checked rather than the list as a whole.
        let list = windows_ui_language().expect("Windows should report a display language");
        let tags: Vec<&str> = list.lines().map(str::trim).filter(|t| !t.is_empty()).collect();
        assert!(!tags.is_empty(), "no display language in {list:?}");
        for tag in tags {
            assert!(tag.split('-').all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_alphanumeric())), "not a locale tag: {tag:?}");
        }
    }

    #[test]
    fn preferences_resolve_with_fallback() {
        assert_eq!(Lang::from_pref("ja"), JA());
        assert_eq!(Lang::from_pref("JA"), JA());
        assert_eq!(Lang::from_pref("en"), Lang::EN);
        // `auto` and unknown codes follow the system (English under test).
        assert_eq!(Lang::from_pref(AUTO), Lang::EN);
        assert_eq!(Lang::from_pref("xx-unknown"), Lang::EN);
        assert_eq!(normalize_pref("JA"), Some("ja"));
        assert_eq!(normalize_pref("Auto"), Some(AUTO));
        assert_eq!(normalize_pref("xx"), None);
    }

    #[test]
    fn lookups_fall_back_to_english() {
        assert_eq!(tr(JA(), "no such label"), "no such label");
        assert_eq!(tr(Lang::EN, "File"), "File");
        assert_eq!(tr(JA(), "File"), "ファイル");
        assert_eq!(tr(JA(), "日本語の文書.pdf"), "日本語の文書.pdf");
        assert_eq!(tr_id(JA(), "no.such.id", "File"), "ファイル");
        assert_eq!(tr_ctx(JA(), "no such context", "File"), "ファイル");
        assert!(has(JA(), "Save") && !has(Lang::EN, "Save"));
    }

    #[test]
    fn catalog_kinds_are_parsed_and_looked_up() {
        let (c, errors) = Catalog::parse(
            "# c\n\tHello\tこんにちは\n@id\tfile.save\t保存する\nmenu\tWindows\tウィンドウ群\n@plural\t{n} file|{n} files\t{n} 個\n\n",
            1,
        );
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(c.plain("Hello"), Some("こんにちは"));
        assert_eq!(c.id("file.save"), Some("保存する"));
        assert_eq!(c.contextual("menu", "Windows"), Some("ウィンドウ群"));
        assert_eq!(c.contextual("other", "Windows"), None);
        assert_eq!(c.plural("{n} file", "{n} files", 0), Some("{n} 個"));
        assert_eq!(c.plural("{n} file", "{n} files", 5), Some("{n} 個"), "an index past the forms clamps");
    }

    #[test]
    fn malformed_lines_are_reported_not_fatal() {
        let (entries, errors) = parse_entries("\tok\tはい\nno tabs here\n\tonly\n\ta\tb\tc\textra\n\t\tempty source\n", 1);
        assert_eq!(entries.len(), 1);
        assert_eq!(errors.len(), 4, "{errors:?}");
        let (entries, errors) = parse_entries("\ta\\tb\tx\\ny\\\\z\n", 1);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(entries, [Entry { context: String::new(), source: "a\tb".into(), translation: "x\ny\\z".into() }]);
    }

    #[test]
    fn strict_validation_skips_only_the_bad_line() {
        for (text, forms, error) in [
            ("\tFile", 1, "context<TAB>"),
            ("\tFile\t", 1, "nonempty"),
            ("\tFile\\q\tFiles", 1, "unknown escape"),
            ("\tFile\tFiles\\", 1, "trailing backslash"),
            ("\tOpen…\tOpen", 1, "ellipsis"),
            ("\tOpen {file}\tOpen {name}", 1, "placeholders"),
            ("@plural\tpage\tPages", 1, "one|other"),
            ("@plural\t{n} page|{n} pages\t{n} pages", 2, "2 nonempty"),
            ("@plural\t{n} page|{n} pages\tPages", 1, "placeholders"),
            ("@unknown\tFile\tFiles", 1, "reserved context"),
        ] {
            // The bad line comes first; the good line after it still loads.
            let (catalog, errors) = Catalog::parse(&format!("{text}\n\tSave\t保存\n"), forms);
            assert_eq!(errors.len(), 1, "{text:?}: {errors:?}");
            assert!(errors[0].starts_with("line 1: ") && errors[0].contains(error), "{errors:?}");
            assert_eq!(catalog.plain("Save"), Some("保存"), "{text:?}");
        }
        let (catalog, errors) = Catalog::parse("\tFile\tファイル\n\tFile\t別\n", 1);
        assert!(errors.len() == 1 && errors[0].contains("line 2: duplicate"), "{errors:?}");
        assert_eq!(catalog.plain("File"), Some("ファイル"), "the first entry wins");
    }

    #[test]
    fn catalogs_handle_escapes_crlf_and_distinct_contexts() {
        let (catalog, errors) =
            Catalog::parse("# comment\r\n\r\n\tLine\\nTab\\tPath\\\\\tOther\\nTab\\tPath\\\\\r\nweight\tLight\tThin\r\ntheme\tLight\tPale\r\n", 1);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(catalog.plain("Line\nTab\tPath\\"), Some("Other\nTab\tPath\\"));
        assert_eq!(catalog.contextual("weight", "Light"), Some("Thin"));
        assert_eq!(catalog.contextual("theme", "Light"), Some("Pale"));
    }

    #[test]
    fn broken_catalog_falls_back_to_english() {
        static BROKEN: LangInfo = LangInfo {
            code: "broken",
            name: "Broken test catalog",
            source: "malformed\n\\",
            plural: plural_none,
            rtl: false,
            catalog: OnceLock::new(),
        };
        assert_eq!(tr(Lang(&BROKEN), "File"), "File");
        assert_eq!(trn(Lang(&BROKEN), 2, "{n} page", "{n} pages"), "2 pages");
    }

    #[test]
    fn contextual_id_and_plural_lookups_have_english_fallbacks() {
        static TEST: LangInfo = LangInfo {
            code: "test",
            name: "Test catalog",
            source: "\tLight\tPlain light\nweight\tLight\tThin\n@id\tfile.open\tOpen dialog…\n@plural\t{n} page|{n} pages\tOne page: {n}|Many pages: {n}\n",
            plural: plural_one_other,
            rtl: false,
            catalog: OnceLock::new(),
        };
        let l = Lang(&TEST);
        assert_eq!(tr_ctx(l, "weight", "Light"), "Thin");
        assert_eq!(tr_ctx(l, "theme", "Light"), "Plain light");
        assert_eq!(tr_ctx(l, "theme", "Unknown"), "Unknown");
        assert_eq!(tr_id(l, "file.open", "Reworded English…"), "Open dialog…");
        assert_eq!(tr_id(l, "unknown", "Light"), "Plain light");
        assert_eq!(tr_id(l, "unknown", "Unknown"), "Unknown");
        assert_eq!(trn(l, 1, "{n} page", "{n} pages"), "One page: 1");
        assert_eq!(trn(l, 0, "{n} page", "{n} pages"), "Many pages: 0");
        assert_eq!(trn(l, u64::MAX, "{n} page", "{n} pages"), format!("Many pages: {}", u64::MAX));
    }

    #[test]
    fn formatting_reorders_parameters_without_reinterpreting_user_text() {
        let args = [("file", "日本語-{n}.pdf"), ("n", "2")];
        assert_eq!(fmt("{n}: {file}; {unknown}", &args), "2: 日本語-{n}.pdf; {unknown}");
        assert_eq!(fmt("{file} / {file}", &args), "日本語-{n}.pdf / 日本語-{n}.pdf");
        assert_eq!(fmt("text {unfinished", &args), "text {unfinished");
        assert_eq!(fmt("}{n}{", &args), "}2{");
        assert_eq!(fmt("unchanged", &[]), "unchanged");
    }

    /// #103: the missing-OCR-models messages are translated under their current English text
    /// (catalogs once keyed an older `PRINTCRAFT_MODELS` wording, which never matched).
    #[test]
    fn missing_ocr_models_messages_are_translated() {
        let error = pdfcraft_engine::ocr::OcrError::NoModels.to_string();
        let notice = "Text recognition isn't installed: its model files are missing. Reinstall PdfCraft, or set PDFCRAFT_MODELS to the folder that holds them.";
        assert!(include_str!("../ocr_ui.rs").contains(notice), "keep in step with ocr_ui.rs");
        for code in ["bg", "de", "es", "fr", "hu", "ja", "ru", "te", "uk", "zh-hans", "zh-hant"] {
            let lang = Lang::from_code(code).expect("registered");
            assert!(has(lang, notice) && has(lang, &error), "{code}");
        }
        for lang in &LANGUAGES {
            assert!(!lang.source.contains("PRINTCRAFT"), "{}", lang.code);
        }
    }

    /// Japanese translates every registered command and every All tools group, section and item.
    #[test]
    fn japanese_covers_commands_and_catalogue() {
        for command in pdfcraft_engine::commands::COMMANDS {
            assert!(has(JA(), command.label), "missing command: {}", command.label);
        }
        for group in pdfcraft_engine::catalog::TOOL_GROUPS {
            assert!(has(JA(), group.label), "missing group: {}", group.label);
            for section in group.sections {
                assert!(has(JA(), section.title), "missing section: {}", section.title);
                for item in section.items {
                    assert!(has(JA(), item.label), "missing item: {}", item.label);
                }
            }
        }
    }

    /// Every `tl!("…")` literal in the UI source (test modules aside), unescaped.
    /// Every label Shared Field Properties shows, including those it passes through a variable
    /// (flag labels, errors, the font label), exists in each full catalog.
    #[test]
    fn bulk_field_properties_labels_are_in_every_full_catalog() {
        let source = include_str!("../bulk_fields.rs");
        // The string literal starting right after `at`, unescaped.
        fn literal(rest: &str) -> Option<String> {
            let mut out = String::new();
            let mut chars = rest.chars();
            while let Some(c) = chars.next() {
                match c {
                    '"' => return Some(out),
                    '\\' => out.push(chars.next()?),
                    c => out.push(c),
                }
            }
            None
        }
        let mut labels = std::collections::BTreeSet::new();
        for marker in ["tl!(\"", "Err(\"", "ok_or(\"", "|| \"", "MISSING: &str = \"", "i18n::t(\"", "_ => \""] {
            for (at, _) in source.match_indices(marker) {
                labels.extend(source.get(at + marker.len()..).and_then(literal));
            }
        }
        // Flag labels: `(field_flags::NAME, "Label", inverted)`.
        for (at, _) in source.match_indices("(field_flags::") {
            let line = source.get(at..).and_then(|s| s.lines().next()).unwrap_or_default();
            if let Some(q) = line.find(", \"") {
                labels.extend(line.get(q + 3..).and_then(literal));
            }
        }
        labels.retain(|l| l.chars().any(char::is_alphabetic) && !l.contains("credits") && !l.starts_with("bulk"));
        for label in ["Mixed", "Custom font", "The document is no longer open."] {
            labels.insert(label.to_string());
        }
        assert!(labels.len() > 20 && labels.contains("Comb of characters"), "the scan found the dialog's labels: {labels:?}");
        for code in ["bg", "de", "es", "fr", "hu", "it", "ja", "pt-br", "ru", "te", "uk", "zh-hans", "zh-hant"] {
            let lang = Lang::from_code(code).expect("registered");
            let missing: Vec<_> = labels.iter().filter(|l| !has(lang, l)).collect();
            assert!(missing.is_empty(), "{code} lacks Shared Field Properties labels: {missing:#?}");
        }
    }

    fn ui_literals() -> std::collections::BTreeSet<String> {
        let mut stack = vec![std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
        let mut literals = std::collections::BTreeSet::new();
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(dir).expect("UI source directory") {
                let path = entry.expect("UI source entry").path();
                if path.is_dir() {
                    if path.file_name().is_some_and(|name| name != "i18n") {
                        stack.push(path);
                    }
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    let source = std::fs::read_to_string(path).expect("UI source file").replace("\r\n", "\n");
                    let mut rest = source.split("#[cfg(test)]\nmod ").next().unwrap_or_default();
                    while let Some((_, after)) = rest.split_once("tl!(\"") {
                        let mut escaped = false;
                        let end = after
                            .char_indices()
                            .find_map(|(i, c)| {
                                if c == '"' && !escaped {
                                    return Some(i);
                                }
                                escaped = c == '\\' && !escaped;
                                None
                            })
                            .expect("closed tl! literal");
                        let (raw, tail) = after.split_at(end);
                        if tail.starts_with("\")") {
                            let label: String = serde_json::from_str(&format!("\"{raw}\"")).expect("UI literal escapes");
                            literals.insert(label);
                        }
                        rest = tail.strip_prefix('"').expect("closing quote");
                    }
                }
            }
        }
        literals
    }

    /// Japanese translates every `tl!("…")` literal in the UI source, so a new string can't ship in
    /// English by accident (the same check French, German, Russian and Simplified Chinese have).
    #[test]
    fn japanese_covers_ui_literals() {
        let literals = ui_literals();
        assert!(literals.len() > 900, "source scan found only {} literals", literals.len());
        let missing: Vec<_> = literals.iter().filter(|label| !has(JA(), label)).collect();
        assert!(missing.is_empty(), "untranslated Japanese UI literals: {missing:#?}");
    }

    /// Italian translates every registered command and every All tools group, section and item.
    #[test]
    fn italian_covers_commands_and_catalogue() {
        let it = Lang::from_code("it").expect("it registered");
        for command in pdfcraft_engine::commands::COMMANDS {
            assert!(has(it, command.label), "missing command: {}", command.label);
            if let Some(menu) = command.menu {
                assert!(has(it, menu), "missing menu: {menu}");
            }
        }
        for group in pdfcraft_engine::catalog::TOOL_GROUPS {
            assert!(has(it, group.label), "missing group: {}", group.label);
            for section in group.sections {
                assert!(has(it, section.title), "missing section: {}", section.title);
                for item in section.items {
                    assert!(has(it, item.label), "missing item: {}", item.label);
                }
            }
        }
    }

    /// New tl!("literal") labels must not silently fall back to English in Italian.
    #[test]
    fn italian_covers_ui_literals() {
        let it = Lang::from_code("it").expect("it registered");
        let literals = ui_literals();
        assert!(literals.len() > 900, "source scan found only {} literals", literals.len());
        let missing: Vec<_> = literals.iter().filter(|label| !has(it, label)).collect();
        assert!(missing.is_empty(), "untranslated Italian UI literals: {missing:#?}");
    }

    /// With craft-fonts, every Japanese translation has glyphs: with all interface faces (desktop)
    /// and with BIZ UDPGothic Regular alone (the web build's only Japanese face).
    #[test]
    fn japanese_labels_have_glyphs() {
        let craft = pdfcraft_fonts::ui_japanese_fonts();
        let Some(web) = craft.iter().find(|f| f.family == "BIZ UDPGothic" && f.style == "Regular") else {
            eprintln!("skipping Japanese glyph checks: build with CRAFT_FONTS_DIR to run them");
            return;
        };
        let (entries, _) = parse_entries(JA().0.source, 1);
        let labels: String = entries.iter().flat_map(|e| e.translation.chars()).chain(JA().name().chars()).filter(|c| !c.is_control()).collect();
        let web_name = web.name();
        let mut web_only = crate::theme::font_definitions();
        for family in web_only.families.values_mut() {
            family.retain(|name| !pdfcraft_fonts::CRAFT_FONTS.iter().any(|face| face.name() == *name) || *name == web_name);
        }
        use egui::epaint::text::{Fonts, TextOptions};
        for (build, defs) in [("desktop", crate::theme::font_definitions()), ("web", web_only)] {
            let mut fonts = Fonts::new(TextOptions::default(), defs);
            for id in [egui::FontId::proportional(13.0), egui::FontId::monospace(13.0), crate::theme::medium(13.0), crate::theme::semibold(17.0)] {
                assert!(fonts.has_glyphs(&id, &labels), "{build}: {id:?} lacks a Japanese label glyph");
            }
        }
    }

    #[test]
    fn history_labels_keep_captured_names() {
        set_current(JA());
        assert_eq!(action_label("Insert pages from Save {e}.pdf"), "Save {e}.pdf からページを挿入");
        assert_eq!(command_label("Undo Insert pages from Save {e}.pdf"), "取り消し Save {e}.pdf からページを挿入");
        assert_eq!(action_label("Untranslated custom action"), "Untranslated custom action");
        assert_eq!(menu_label("file.open", "Open…"), "開く…");
        set_current(Lang::EN);
        assert_eq!(command_label("Undo Fill in Save {e}"), "Undo Fill in Save {e}");
    }

    #[test]
    fn current_language_is_per_thread() {
        set_current(JA());
        assert_eq!(t("File"), "ファイル");
        std::thread::spawn(|| assert_eq!(current(), Lang::EN)).join().unwrap();
        set_current(Lang::EN);
        assert_eq!(t("File"), "File");
    }

    #[test]
    fn plurals_and_placeholders() {
        assert_eq!(trn(Lang::EN, 1, "{n} item", "{n} items"), "1 item");
        assert_eq!(trn(Lang::EN, 0, "{n} item", "{n} items"), "0 items");
        assert_eq!(trn(Lang::EN, 7, "{n} item", "{n} items"), "7 items");
        // No `@plural` entry yet: Japanese falls back to the English forms.
        assert_eq!(trn(JA(), 7, "{n} item", "{n} items"), "7 items");
        assert_eq!(fmt("{b} before {a}", &[("a", "x"), ("b", "y"), ("c", "z")]), "y before x");
        assert_eq!(fmt("{missing}", &[]), "{missing}");
        assert_eq!(placeholders("a {x} b {y} {"), ["x", "y"]);
    }

    #[test]
    fn language_persists_and_invalid_input_keeps_current_language() {
        let mut app = crate::PdfCraftApp::default();
        assert_eq!(app.language, AUTO);
        app.set_option("language", "JA").unwrap();
        assert_eq!(app.language, "ja");
        assert!(app.set_option("language", "xx").is_err());
        assert_eq!(app.language, "ja");
        let mut restored = crate::PdfCraftApp::default();
        restored.restore(&app.persist());
        assert_eq!(restored.language, "ja");
        restored.restore(r#"{"language":"xx"}"#);
        assert_eq!(restored.language, "ja");
        app.set_option("language", AUTO).unwrap();
        assert_eq!(app.language, AUTO);
        // Settings written before the system-locale default (no key) follow the system.
        let mut legacy = crate::PdfCraftApp::default();
        legacy.restore("{}");
        assert_eq!(legacy.language, AUTO);
    }

    #[test]
    fn brazilian_portuguese_is_registered() {
        let pt = Lang::from_code("pt-br").expect("pt-br registered");
        assert_eq!(pt.name(), "Português (Brasil)");
        assert_eq!(Lang::from_code("PT-BR"), Some(pt));
        assert_eq!(normalize_pref("pt-BR"), Some("pt-br"));
        assert_eq!(normalize_pref("pt"), None, "only exact codes are preferences");
        assert_eq!(lang_from_tag("pt_BR.UTF-8"), Some(pt));
        assert_eq!(lang_from_tag("pt_PT"), Some(pt));
        assert_eq!(tr(pt, "File"), "Arquivo");
        assert_eq!(tr(pt, "Save as…"), "Salvar como…");
        assert_eq!(tr(pt, "Arquivo do usuário.pdf"), "Arquivo do usuário.pdf");
        assert_eq!((0..=3).map(|n| (pt.0.plural)(n)).collect::<Vec<_>>(), [0, 0, 1, 1]);
        let mut app = crate::PdfCraftApp::default();
        app.set_option("language", "pt-br").unwrap();
        assert_eq!(app.language, "pt-br");
        let mut restored = crate::PdfCraftApp::default();
        restored.restore(&app.persist());
        assert_eq!(restored.language, "pt-br");
    }

    /// Brazilian Portuguese translates every registered command and every All tools group, section and item.
    #[test]
    fn brazilian_portuguese_covers_commands_and_catalogue() {
        let pt = Lang::from_code("pt-br").expect("pt-br registered");
        for command in pdfcraft_engine::commands::COMMANDS {
            assert!(has(pt, command.label), "missing command: {}", command.label);
            if let Some(menu) = command.menu {
                assert!(has(pt, menu), "missing menu: {menu}");
            }
        }
        for group in pdfcraft_engine::catalog::TOOL_GROUPS {
            assert!(has(pt, group.label), "missing group: {}", group.label);
            for section in group.sections {
                assert!(has(pt, section.title), "missing section: {}", section.title);
                for item in section.items {
                    assert!(has(pt, item.label), "missing item: {}", item.label);
                }
            }
        }
    }

    /// Check direct lookups and multiline literals as well as ordinary tl!("literal") calls,
    /// following the complete-catalog check for Simplified Chinese.
    #[test]
    fn brazilian_portuguese_covers_ui_literals() {
        let pt = Lang::from_code("pt-br").expect("pt-br registered");
        let mut stack = vec![std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
        let mut literals = std::collections::BTreeSet::new();
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(dir).expect("UI source directory") {
                let path = entry.expect("UI source entry").path();
                if path.is_dir() {
                    if path.file_name().is_some_and(|name| name != "i18n") {
                        stack.push(path);
                    }
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    let source = std::fs::read_to_string(path).expect("UI source file").replace("\r\n", "\n").replace("crate::i18n::t(", "tl!(");
                    let mut rest = source.split("#[cfg(test)]\nmod ").next().unwrap_or_default();
                    while let Some((_, after)) = rest.split_once("tl!(") {
                        let after = after.trim_start();
                        let Some(after) = after.strip_prefix('"') else {
                            rest = after;
                            continue;
                        };
                        let mut escaped = false;
                        let end = after
                            .char_indices()
                            .find_map(|(i, c)| {
                                if c == '"' && !escaped {
                                    return Some(i);
                                }
                                escaped = c == '\\' && !escaped;
                                None
                            })
                            .expect("closed tl! literal");
                        let (raw, tail) = after.split_at(end);
                        let closing = tail.strip_prefix('"').expect("closing quote").trim_start();
                        let closing = closing.strip_prefix(',').unwrap_or(closing).trim_start();
                        if closing.starts_with(')') {
                            let label: String = serde_json::from_str(&format!("\"{raw}\"")).expect("UI literal escapes");
                            literals.insert(label);
                        }
                        rest = tail.strip_prefix('"').expect("closing quote");
                    }
                }
            }
        }
        assert!(literals.len() > 900, "source scan found only {} literals", literals.len());
        let missing: Vec<_> = literals.iter().filter(|label| !has(pt, label)).collect();
        assert!(missing.is_empty(), "untranslated Brazilian Portuguese UI literals: {missing:#?}");
    }

    #[test]
    fn spanish_is_registered() {
        let es = Lang::from_code("es").expect("es registered");
        assert_eq!(es.name(), "Español");
        assert_eq!(normalize_pref("ES"), Some("es"));
        assert_eq!(lang_from_tag("es_ES.UTF-8"), Some(es));
        assert_eq!(lang_from_tag("es-MX"), Some(es));
        assert_eq!(first_supported("es-ES\r\nen-US"), Some(es));
        assert_eq!(tr(es, "File"), "Archivo");
        assert_eq!(tr(es, "Save as…"), "Guardar como…");
        assert_eq!(tr(es, "Informe del usuario.pdf"), "Informe del usuario.pdf");
        assert_eq!(trn(es, 1, "{n} page", "{n} pages"), "1 página");
        assert_eq!(trn(es, 3, "{n} page", "{n} pages"), "3 páginas");
        let mut app = crate::PdfCraftApp::default();
        app.set_option("language", "es").unwrap();
        assert_eq!(app.language, "es");
        let mut restored = crate::PdfCraftApp::default();
        restored.restore(&app.persist());
        assert_eq!(restored.language, "es");
    }

    /// Spanish translates every registered command and every All tools group, section and item.
    #[test]
    fn spanish_covers_commands_and_catalogue() {
        let es = Lang::from_code("es").expect("es registered");
        for command in pdfcraft_engine::commands::COMMANDS {
            assert!(has(es, command.label), "missing command: {}", command.label);
        }
        for group in pdfcraft_engine::catalog::TOOL_GROUPS {
            assert!(has(es, group.label), "missing group: {}", group.label);
            for section in group.sections {
                assert!(has(es, section.title), "missing section: {}", section.title);
                for item in section.items {
                    assert!(has(es, item.label), "missing item: {}", item.label);
                }
            }
        }
    }

    #[test]
    fn telugu_is_registered() {
        let te = Lang::from_code("te").expect("te registered");
        assert_eq!(te.name(), "తెలుగు");
        assert_eq!(normalize_pref("TE"), Some("te"));
        assert_eq!(lang_from_tag("te_IN.UTF-8"), Some(te));
        assert_eq!(first_supported("te-IN\r\nen-US"), Some(te));
        assert_eq!(tr(te, "File"), "ఫైల్");
        assert_eq!(tr(te, "Save as…"), "వేరే పేరుతో సేవ్ చేయి…");
        assert_eq!(tr(te, "నివేదిక.pdf"), "నివేదిక.pdf");
        assert_eq!(trn(te, 1, "{n} page", "{n} pages"), "1 పేజీ");
        assert_eq!(trn(te, 3, "{n} page", "{n} pages"), "3 పేజీలు");
        let mut app = crate::PdfCraftApp::default();
        app.set_option("language", "te").unwrap();
        assert_eq!(app.language, "te");
        let mut restored = crate::PdfCraftApp::default();
        restored.restore(&app.persist());
        assert_eq!(restored.language, "te");
    }

    /// Telugu puts the verb last, so it translates "Undo {action}" as a whole phrase; a catalog
    /// without that entry (Spanish) keeps the verb in front of the action.
    #[test]
    fn undo_and_redo_labels_use_the_catalog_phrase_when_there_is_one() {
        let te = Lang::from_code("te").expect("te registered");
        set_current(te);
        assert_eq!(command_label("Undo"), "రద్దు చేయి");
        assert_eq!(command_label("Undo Untranslated custom action"), "రద్దు చేయి: Untranslated custom action");
        assert_eq!(command_label("Redo Untranslated custom action"), "మళ్లీ చేయి: Untranslated custom action");
        let es = Lang::from_code("es").expect("es registered");
        set_current(es);
        assert_eq!(command_label("Undo Untranslated custom action"), format!("{} Untranslated custom action", tr(es, "Undo")));
        set_current(Lang::EN);
        assert_eq!(command_label("Undo Untranslated custom action"), "Undo Untranslated custom action");
    }

    /// A label that reads differently by use has its own contextual entry where a language needs
    /// one; a language without it falls back to the plain translation.
    #[test]
    fn contextual_labels_fall_back_to_the_plain_translation() {
        let te = Lang::from_code("te").expect("te registered");
        assert_eq!(tr_ctx(te, "signature pad", "Type"), "టైప్ చేయి");
        assert_eq!(tr(te, "Type"), "రకం");
        let es = Lang::from_code("es").expect("es registered");
        assert_eq!(tr_ctx(es, "signature pad", "Type"), tr(es, "Type"));
    }

    /// Telugu translates every registered command and every All tools group, section and item.
    #[test]
    fn telugu_covers_commands_and_catalogue() {
        let te = Lang::from_code("te").expect("te registered");
        for command in pdfcraft_engine::commands::COMMANDS {
            assert!(has(te, command.label), "missing command: {}", command.label);
        }
        for group in pdfcraft_engine::catalog::TOOL_GROUPS {
            assert!(has(te, group.label), "missing group: {}", group.label);
            for section in group.sections {
                assert!(has(te, section.title), "missing section: {}", section.title);
                for item in section.items {
                    assert!(has(te, item.label), "missing item: {}", item.label);
                }
            }
        }
    }

    /// With a craft-fonts Telugu face, every Telugu translation has glyphs: with all interface
    /// faces (desktop) and with the Telugu face as the only craft-fonts face (the web build keeps
    /// it; Telugu labels must not need a Japanese or Arabic face).
    #[test]
    fn telugu_labels_have_glyphs() {
        let telugu: Vec<String> = pdfcraft_fonts::ui_telugu_fonts().iter().map(|f| f.name()).collect();
        if telugu.is_empty() {
            eprintln!("skipping Telugu glyph checks: build with CRAFT_FONTS_DIR and a Telugu face to run them");
            return;
        }
        let te = Lang::from_code("te").expect("te registered");
        let (entries, _) = parse_entries(te.0.source, te.0.plural_forms());
        let labels: String = entries.iter().flat_map(|e| e.translation.chars()).chain(te.name().chars()).filter(|c| !c.is_control()).collect();
        let mut telugu_only = crate::theme::font_definitions();
        for family in telugu_only.families.values_mut() {
            family.retain(|name| !pdfcraft_fonts::CRAFT_FONTS.iter().any(|face| face.name() == *name) || telugu.contains(name));
        }
        use egui::epaint::text::{Fonts, TextOptions};
        for (build, defs) in [("all faces", crate::theme::font_definitions()), ("Telugu face only", telugu_only)] {
            let mut fonts = Fonts::new(TextOptions::default(), defs);
            for id in [egui::FontId::proportional(13.0), egui::FontId::monospace(13.0), crate::theme::medium(13.0), crate::theme::semibold(17.0)] {
                assert!(fonts.has_glyphs(&id, &labels), "{build}: {id:?} lacks a Telugu label glyph");
            }
        }
    }

    /// Czech covers every registered menu title and menu command label (#68), keeps ellipses, and
    /// only leaves untranslated what is deliberately the same in Czech.
    #[test]
    fn czech_covers_every_menu_label() {
        let cs = Lang::from_code("cs").expect("cs registered");
        assert_eq!(cs.name(), "Čeština");
        assert_eq!(tr(cs, "File"), "Soubor");
        assert_eq!(tr(cs, "Žluťoučký kůň.pdf"), "Žluťoučký kůň.pdf");
        assert_eq!((0..=6).map(|n| (cs.0.plural)(n)).collect::<Vec<_>>(), [2, 0, 1, 1, 1, 2, 2]);
        for spec in pdfcraft_engine::commands::COMMANDS {
            let Some(menu) = spec.menu else { continue };
            assert!(has(cs, menu), "Czech lacks the menu title {menu:?}");
            assert!(has(cs, spec.label), "Czech lacks the {menu} menu label {:?} ({})", spec.label, spec.id);
        }
        const KEEP_AS_IS: &[&str] = &["OK"];
        let (entries, _) = parse_entries(cs.0.source, cs.0.plural_forms());
        for e in &entries {
            let key_cap = e.context == "key" && KEY_CAP_NAMES.contains(&e.source.as_str());
            assert!(e.source != e.translation || KEEP_AS_IS.contains(&e.source.as_str()) || key_cap, "{:?} is untranslated", e.source);
            assert!(!e.translation.contains("..."), "use … rather than three dots: {:?}", e.translation);
            assert_eq!(e.translation.trim(), e.translation, "stray whitespace: {:?}", e.translation);
        }
        let mut app = crate::PdfCraftApp::default();
        app.set_option("language", "cs").unwrap();
        assert!(app.set_option("language", "cz").is_err());
        assert_eq!(app.language, "cs");
        let mut restored = crate::PdfCraftApp::default();
        restored.restore(&app.persist());
        assert_eq!(restored.language, "cs");
    }

    #[test]
    fn simplified_chinese_is_registered() {
        let zh = Lang::from_code("zh-hans").expect("zh-hans registered");
        assert_eq!(zh.name(), "简体中文");
        assert_eq!(normalize_pref("zh-Hans"), Some("zh-hans"));
        assert_eq!(normalize_pref("zh"), None, "only exact codes are preferences");
        assert_eq!(tr(zh, "File"), "文件");
        assert_eq!(tr(zh, "报告.pdf"), "报告.pdf");
        for menu in ["File", "Edit", "Pages", "View", "Help", "Preferences…", "Interface language"] {
            assert!(has(zh, menu), "missing {menu}");
        }
    }

    /// Like PhotoCraft's complete-catalog gates, derive coverage from the UI, not another language.
    #[test]
    fn simplified_chinese_covers_commands_and_catalogue() {
        let zh = Lang::from_code("zh-hans").expect("zh-hans registered");
        for command in pdfcraft_engine::commands::COMMANDS {
            assert!(has(zh, command.label), "missing command: {}", command.label);
            if let Some(menu) = command.menu {
                assert!(has(zh, menu), "missing menu: {menu}");
            }
        }
        for group in pdfcraft_engine::catalog::TOOL_GROUPS {
            assert!(has(zh, group.label), "missing group: {}", group.label);
            for section in group.sections {
                assert!(has(zh, section.title), "missing section: {}", section.title);
                for item in section.items {
                    assert!(has(zh, item.label), "missing item: {}", item.label);
                }
            }
        }
    }

    #[test]
    fn simplified_chinese_covers_about_credit_labels() {
        let zh = Lang::from_code("zh-hans").expect("zh-hans registered");
        for label in ["About", "Contributors", "Models"].into_iter().chain(crate::credits::MODEL_COLUMNS) {
            assert!(has(zh, label), "missing About label: {label}");
        }
        for mode in crate::credits::NameMode::ALL {
            assert!(has(zh, mode.label()), "missing name mode: {}", mode.label());
        }
        for key in crate::credits::SortKey::ALL {
            let (label, header) = key.label();
            for text in [label, header] {
                assert!(has(zh, text), "missing contributor sort/header: {text}");
            }
        }
    }

    /// Direct tl! and i18n::t labels must not silently fall back to English in the complete catalog.
    #[test]
    fn simplified_chinese_covers_ui_literals() {
        let zh = Lang::from_code("zh-hans").expect("zh-hans registered");
        let mut stack = vec![std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
        let mut literals = std::collections::BTreeSet::new();
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(dir).expect("UI source directory") {
                let path = entry.expect("UI source entry").path();
                if path.is_dir() {
                    // Catalog implementation and its test-only lookup examples are not UI labels.
                    if path.file_name().is_some_and(|name| name != "i18n") {
                        stack.push(path);
                    }
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    let source = std::fs::read_to_string(path).expect("UI source file").replace("\r\n", "\n").replace("crate::i18n::t(", "tl!(");
                    let mut rest = source.split("#[cfg(test)]\nmod ").next().unwrap_or_default();
                    while let Some((_, after)) = rest.split_once("tl!(") {
                        let after = after.trim_start();
                        let Some(after) = after.strip_prefix('"') else {
                            rest = after;
                            continue;
                        };
                        let mut escaped = false;
                        let end = after
                            .char_indices()
                            .find_map(|(i, c)| {
                                if c == '"' && !escaped {
                                    return Some(i);
                                }
                                escaped = c == '\\' && !escaped;
                                None
                            })
                            .expect("closed tl! literal");
                        let (raw, tail) = after.split_at(end);
                        let closing = tail.strip_prefix('"').expect("closing quote").trim_start();
                        let closing = closing.strip_prefix(',').unwrap_or(closing).trim_start();
                        if closing.starts_with(')') {
                            // Current UI literals use the shared Rust/JSON string escapes.
                            let label: String = serde_json::from_str(&format!("\"{raw}\"")).expect("UI literal escapes");
                            literals.insert(label);
                        }
                        rest = tail.strip_prefix('"').expect("closing quote");
                    }
                }
            }
        }
        assert!(literals.len() > 900, "source scan found only {} literals", literals.len());
        let missing: Vec<_> = literals.iter().filter(|label| !has(zh, label)).collect();
        assert!(missing.is_empty(), "untranslated Simplified Chinese UI literals: {missing:#?}");
    }

    #[test]
    fn simplified_chinese_history_and_diagnostics_preserve_user_values() {
        let zh = Lang::from_code("zh-hans").expect("zh-hans registered");
        set_current(zh);
        assert_eq!(command_label("Undo Insert pages from 报告 {n}.pdf"), "撤销 从 报告 {n}.pdf 插入页面");
        assert_eq!(command_label("Redo Fill in 联系人 {key}"), "重做 填写 联系人 {key}");
        assert_eq!(action_label("Change Title"), "更改标题");
        assert_eq!(action_label("Custom action {n}"), "Custom action {n}");
        assert_eq!(fmt(t("This page couldn't be displayed.\n{e}"), &[("e", "OS error {n}")]), "无法显示此页面。\nOS error {n}");
        assert_eq!(fmt(t("{n} pages selected"), &[("n", "3")]), "已选择 3 页");
        assert_eq!(tr(zh, "CheckBox"), "复选框");
        assert_eq!(tr(zh, "pages"), "页");
        assert_eq!(trn(zh, 1, "{n} page", "{n} pages"), "1 页");
        assert_eq!(trn(zh, 0, "{n} field", "{n} fields"), "0 个字段");
        assert_eq!(trn(zh, 2, "{n} field", "{n} fields"), "2 个字段");
        assert_eq!(fmt(t("{n} contributors · {c} commits"), &[("n", "2"), ("c", "1,234")]), "2 位贡献者 · 1,234 次提交");
        let contributor = crate::credits::Contributor {
            login: "reader{n}",
            display_name: Some("Save"),
            real_name: None,
            prs: 2,
            commits: 3,
            lines_added: 10,
            lines_deleted: 4,
            binary_added: 1,
            binary_deleted: 0,
            first_commit: "2026-10-01T00:00:00Z",
            last_commit: "2026-10-09T00:00:00Z",
        };
        assert_eq!(
            contributor.summary(),
            "@reader{n}：2 个 PR，3 次提交，新增 10 行 / 删除 4 行（净增 +6 行），新增 1 个 / 删除 0 个二进制资源，2026-10-01 – 2026-10-09"
        );
        assert_eq!(contributor.name(crate::credits::NameMode::DisplayName), "Save");
        set_current(Lang::EN);
    }

    #[test]
    fn traditional_chinese_is_registered() {
        let zh = Lang::from_code("zh-hant").expect("zh-hant registered");
        assert_eq!(zh.name(), "繁體中文");
        assert_eq!(normalize_pref("zh-Hant"), Some("zh-hant"));
        assert_eq!(tr(zh, "File"), "檔案");
        assert_eq!(tr(zh, "報告.pdf"), "報告.pdf");
        for command in pdfcraft_engine::commands::COMMANDS {
            assert!(has(zh, command.label), "missing command: {}", command.label);
        }
    }

    #[test]
    fn french_is_registered() {
        let fr = Lang::from_code("fr").expect("fr registered");
        assert_eq!(fr.name(), "Français");
        assert_eq!(normalize_pref("FR"), Some("fr"));
        assert_eq!(lang_from_tag("fr_FR.UTF-8"), Some(fr));
        assert_eq!(lang_from_tag("fr-CA"), Some(fr));
        assert_eq!(first_supported("fr-FR\r\nen-US"), Some(fr));
        assert_eq!(tr(fr, "File"), "Fichier");
        assert_eq!(tr(fr, "Save as…"), "Enregistrer sous…");
        assert_eq!(tr(fr, "Bookmarks"), "Signets");
        assert_eq!(tr(fr, "Layers"), "Calques");
        assert_eq!(tr(fr, "Rapport de l'utilisateur.pdf"), "Rapport de l'utilisateur.pdf");
        assert_eq!((0..=3).map(|n| (fr.0.plural)(n)).collect::<Vec<_>>(), [0, 0, 1, 1]);
        assert_eq!(trn(fr, 0, "{n} page", "{n} pages"), "0 page");
        assert_eq!(trn(fr, 1, "{n} page", "{n} pages"), "1 page");
        assert_eq!(trn(fr, 2, "{n} page", "{n} pages"), "2 pages");
        assert_eq!(trn(fr, 1, "{n} field", "{n} fields"), "1 champ");
        assert_eq!(trn(fr, 2, "{n} field", "{n} fields"), "2 champs");
        let mut app = crate::PdfCraftApp::default();
        app.set_option("language", "fr").unwrap();
        assert_eq!(app.language, "fr");
        let mut restored = crate::PdfCraftApp::default();
        restored.restore(&app.persist());
        assert_eq!(restored.language, "fr");
    }

    /// French translates every registered command and every All tools group, section and item.
    #[test]
    fn french_covers_commands_and_catalogue() {
        let fr = Lang::from_code("fr").expect("fr registered");
        for command in pdfcraft_engine::commands::COMMANDS {
            assert!(has(fr, command.label), "missing command: {}", command.label);
            if let Some(menu) = command.menu {
                assert!(has(fr, menu), "missing menu: {menu}");
            }
        }
        for group in pdfcraft_engine::catalog::TOOL_GROUPS {
            assert!(has(fr, group.label), "missing group: {}", group.label);
            for section in group.sections {
                assert!(has(fr, section.title), "missing section: {}", section.title);
                for item in section.items {
                    assert!(has(fr, item.label), "missing item: {}", item.label);
                }
            }
        }
    }

    /// New tl!("literal") labels must not silently fall back to English.
    #[test]
    fn french_covers_ui_literals() {
        let fr = Lang::from_code("fr").expect("fr registered");
        let mut stack = vec![std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
        let mut literals = std::collections::BTreeSet::new();
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(dir).expect("UI source directory") {
                let path = entry.expect("UI source entry").path();
                if path.is_dir() {
                    if path.file_name().is_some_and(|name| name != "i18n") {
                        stack.push(path);
                    }
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    let source = std::fs::read_to_string(path).expect("UI source file").replace("\r\n", "\n");
                    let mut rest = source.split("#[cfg(test)]\nmod ").next().unwrap_or_default();
                    while let Some((_, after)) = rest.split_once("tl!(\"") {
                        let mut escaped = false;
                        let end = after
                            .char_indices()
                            .find_map(|(i, c)| {
                                if c == '"' && !escaped {
                                    return Some(i);
                                }
                                escaped = c == '\\' && !escaped;
                                None
                            })
                            .expect("closed tl! literal");
                        let (raw, tail) = after.split_at(end);
                        if tail.starts_with("\")") {
                            let label: String = serde_json::from_str(&format!("\"{raw}\"")).expect("UI literal escapes");
                            literals.insert(label);
                        }
                        rest = tail.strip_prefix('"').expect("closing quote");
                    }
                }
            }
        }
        assert!(literals.len() > 900, "source scan found only {} literals", literals.len());
        let missing: Vec<_> = literals.iter().filter(|label| !has(fr, label)).collect();
        assert!(missing.is_empty(), "untranslated French UI literals: {missing:#?}");
    }

    #[test]
    fn french_history_and_diagnostics_preserve_user_values() {
        let fr = Lang::from_code("fr").expect("fr registered");
        set_current(fr);
        assert_eq!(command_label("Undo Insert pages from Rapport {n}.pdf"), "Annuler Insérer des pages depuis Rapport {n}.pdf");
        assert_eq!(command_label("Redo Fill in Contact {key}"), "Rétablir Remplir Contact {key}");
        assert_eq!(action_label("Change Title"), "Modifier Titre");
        assert_eq!(action_label("Custom action {n}"), "Custom action {n}");
        assert_eq!(fmt(t("This page couldn't be displayed.\n{e}"), &[("e", "OS error {n}")]), "Impossible d'afficher cette page.\nOS error {n}");
        assert_eq!(fmt(t("{n} pages selected"), &[("n", "3")]), "3 pages sélectionnées");
        assert_eq!(tr(fr, "CheckBox"), "Case à cocher");
        set_current(Lang::EN);
    }

    #[test]
    fn german_is_registered() {
        let de = Lang::from_code("de").expect("de registered");
        assert_eq!(de.name(), "Deutsch");
        assert_eq!(normalize_pref("DE"), Some("de"));
        assert_eq!(lang_from_tag("de_DE.UTF-8"), Some(de));
        assert_eq!(lang_from_tag("de-AT"), Some(de));
        assert_eq!(lang_from_tag("de_CH"), Some(de));
        assert_eq!(first_supported("de-DE\r\nen-US"), Some(de));
        assert_eq!(tr(de, "File"), "Datei");
        assert_eq!(tr(de, "Save as…"), "Speichern unter…");
        assert_eq!(tr(de, "Bookmarks"), "Lesezeichen");
        assert_eq!(tr(de, "Layers"), "Ebenen");
        assert_eq!(tr(de, "Bericht des Nutzers.pdf"), "Bericht des Nutzers.pdf");
        assert_eq!((0..=3).map(|n| (de.0.plural)(n)).collect::<Vec<_>>(), [1, 0, 1, 1]);
        assert_eq!(trn(de, 0, "{n} page", "{n} pages"), "0 Seiten");
        assert_eq!(trn(de, 1, "{n} page", "{n} pages"), "1 Seite");
        assert_eq!(trn(de, 2, "{n} page", "{n} pages"), "2 Seiten");
        assert_eq!(trn(de, 1, "{n} field", "{n} fields"), "1 Feld");
        assert_eq!(trn(de, 2, "{n} field", "{n} fields"), "2 Felder");
        let mut app = crate::PdfCraftApp::default();
        app.set_option("language", "de").unwrap();
        assert_eq!(app.language, "de");
        let mut restored = crate::PdfCraftApp::default();
        restored.restore(&app.persist());
        assert_eq!(restored.language, "de");
    }

    /// German translates every registered command and every All tools group, section and item.
    #[test]
    fn german_covers_commands_and_catalogue() {
        let de = Lang::from_code("de").expect("de registered");
        for command in pdfcraft_engine::commands::COMMANDS {
            assert!(has(de, command.label), "missing command: {}", command.label);
            if let Some(menu) = command.menu {
                assert!(has(de, menu), "missing menu: {menu}");
            }
        }
        for group in pdfcraft_engine::catalog::TOOL_GROUPS {
            assert!(has(de, group.label), "missing group: {}", group.label);
            for section in group.sections {
                assert!(has(de, section.title), "missing section: {}", section.title);
                for item in section.items {
                    assert!(has(de, item.label), "missing item: {}", item.label);
                }
            }
        }
    }

    /// New tl!("literal") labels must not silently fall back to English.
    #[test]
    fn german_covers_ui_literals() {
        let de = Lang::from_code("de").expect("de registered");
        let mut stack = vec![std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
        let mut literals = std::collections::BTreeSet::new();
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(dir).expect("UI source directory") {
                let path = entry.expect("UI source entry").path();
                if path.is_dir() {
                    if path.file_name().is_some_and(|name| name != "i18n") {
                        stack.push(path);
                    }
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    let source = std::fs::read_to_string(path).expect("UI source file").replace("\r\n", "\n");
                    let mut rest = source.split("#[cfg(test)]\nmod ").next().unwrap_or_default();
                    while let Some((_, after)) = rest.split_once("tl!(\"") {
                        let mut escaped = false;
                        let end = after
                            .char_indices()
                            .find_map(|(i, c)| {
                                if c == '"' && !escaped {
                                    return Some(i);
                                }
                                escaped = c == '\\' && !escaped;
                                None
                            })
                            .expect("closed tl! literal");
                        let (raw, tail) = after.split_at(end);
                        if tail.starts_with("\")") {
                            let label: String = serde_json::from_str(&format!("\"{raw}\"")).expect("UI literal escapes");
                            literals.insert(label);
                        }
                        rest = tail.strip_prefix('"').expect("closing quote");
                    }
                }
            }
        }
        assert!(literals.len() > 900, "source scan found only {} literals", literals.len());
        let missing: Vec<_> = literals.iter().filter(|label| !has(de, label)).collect();
        assert!(missing.is_empty(), "untranslated German UI literals: {missing:#?}");
    }

    #[test]
    fn tool_names_inside_sentences_are_lowercased_except_in_german() {
        set_current(Lang::EN);
        assert_eq!(in_sentence("Text Box"), "text box");
        set_current(Lang::from_code("de").expect("de registered"));
        assert_eq!(in_sentence("Textfeld"), "Textfeld");
        set_current(Lang::EN);
    }

    #[test]
    fn german_certificate_subject_has_a_distinct_context() {
        let de = Lang::from_code("de").expect("de registered");
        assert_eq!(tr(de, "Subject"), "Thema");
        assert_eq!(tr_ctx(de, "certificate", "Subject"), "Zertifikatsinhaber");
        assert_eq!(tr_ctx(de, "certificate", "Issuer"), tr(de, "Issuer"));
        assert_eq!(tr_ctx(Lang::EN, "certificate", "Subject"), "Subject");
    }

    #[test]
    fn german_history_and_diagnostics_preserve_user_values() {
        let de = Lang::from_code("de").expect("de registered");
        set_current(de);
        assert_eq!(command_label("Undo Insert pages from Rapport {n}.pdf"), "Seiten aus Rapport {n}.pdf einfügen rückgängig machen");
        assert_eq!(command_label("Redo Fill in Contact {key}"), "Contact {key} ausfüllen wiederholen");
        assert_eq!(action_label("Change Title"), "Titel ändern");
        assert_eq!(action_label("Custom action {n}"), "Custom action {n}");
        assert_eq!(
            fmt(t("This page couldn't be displayed.\n{e}"), &[("e", "OS error {n}")]),
            "Diese Seite konnte nicht angezeigt werden.\nOS error {n}"
        );
        assert_eq!(fmt(t("{n} pages selected"), &[("n", "3")]), "3 Seiten ausgewählt");
        assert_eq!(tr(de, "CheckBox"), "Kontrollkästchen");
        set_current(Lang::EN);
    }

    #[test]
    fn russian_is_registered() {
        let ru = Lang::from_code("ru").expect("ru registered");
        assert_eq!(ru.name(), "Русский");
        assert_eq!(normalize_pref("RU"), Some("ru"));
        assert_eq!(lang_from_tag("ru_RU.UTF-8"), Some(ru));
        assert_eq!(lang_from_tag("ru-BY"), Some(ru));
        assert_eq!(first_supported("ru-RU\r\nen-US"), Some(ru));
        assert_eq!(tr(ru, "File"), "Файл");
        assert_eq!(tr(ru, "Save as…"), "Сохранить как…");
        assert_eq!(tr(ru, "Bookmarks"), "Закладки");
        assert_eq!(tr(ru, "Layers"), "Слои");
        assert_eq!(tr(ru, "Rapport de l'utilisateur.pdf"), "Rapport de l'utilisateur.pdf");
        // one (1), few (2-4), many (0, 5+), with 11-14 exception
        assert_eq!((0..=3).map(|n| (ru.0.plural)(n)).collect::<Vec<_>>(), [2, 0, 1, 1]);
        assert_eq!(trn(ru, 0, "{n} page", "{n} pages"), "0 страниц");
        assert_eq!(trn(ru, 1, "{n} page", "{n} pages"), "1 страница");
        assert_eq!(trn(ru, 2, "{n} page", "{n} pages"), "2 страницы");
        assert_eq!(trn(ru, 5, "{n} page", "{n} pages"), "5 страниц");
        assert_eq!(trn(ru, 11, "{n} page", "{n} pages"), "11 страниц");
        assert_eq!(trn(ru, 21, "{n} page", "{n} pages"), "21 страница");
        assert_eq!(trn(ru, 22, "{n} page", "{n} pages"), "22 страницы");
        assert_eq!(trn(ru, 1, "{n} field", "{n} fields"), "1 поле");
        assert_eq!(trn(ru, 2, "{n} field", "{n} fields"), "2 поля");
        assert_eq!(trn(ru, 5, "{n} field", "{n} fields"), "5 полей");
        let mut app = crate::PdfCraftApp::default();
        app.set_option("language", "ru").unwrap();
        assert_eq!(app.language, "ru");
        let mut restored = crate::PdfCraftApp::default();
        restored.restore(&app.persist());
        assert_eq!(restored.language, "ru");
    }

    /// Russian translates every registered command and every All tools group, section and item.
    #[test]
    fn russian_covers_commands_and_catalogue() {
        let ru = Lang::from_code("ru").expect("ru registered");
        for command in pdfcraft_engine::commands::COMMANDS {
            assert!(has(ru, command.label), "missing command: {}", command.label);
            if let Some(menu) = command.menu {
                assert!(has(ru, menu), "missing menu: {menu}");
            }
        }
        for group in pdfcraft_engine::catalog::TOOL_GROUPS {
            assert!(has(ru, group.label), "missing group: {}", group.label);
            for section in group.sections {
                assert!(has(ru, section.title), "missing section: {}", section.title);
                for item in section.items {
                    assert!(has(ru, item.label), "missing item: {}", item.label);
                }
            }
        }
    }

    /// New tl!("literal") labels must not silently fall back to English.
    #[test]
    fn russian_covers_ui_literals() {
        let ru = Lang::from_code("ru").expect("ru registered");
        let mut stack = vec![std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
        let mut literals = std::collections::BTreeSet::new();
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(dir).expect("UI source directory") {
                let path = entry.expect("UI source entry").path();
                if path.is_dir() {
                    if path.file_name().is_some_and(|name| name != "i18n") {
                        stack.push(path);
                    }
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    let source = std::fs::read_to_string(path).expect("UI source file").replace("\r\n", "\n");
                    let mut rest = source.split("#[cfg(test)]\nmod ").next().unwrap_or_default();
                    while let Some((_, after)) = rest.split_once("tl!(\"") {
                        let mut escaped = false;
                        let end = after
                            .char_indices()
                            .find_map(|(i, c)| {
                                if c == '"' && !escaped {
                                    return Some(i);
                                }
                                escaped = c == '\\' && !escaped;
                                None
                            })
                            .expect("closed tl! literal");
                        let (raw, tail) = after.split_at(end);
                        if tail.starts_with("\")") {
                            let label: String = serde_json::from_str(&format!("\"{raw}\"")).expect("UI literal escapes");
                            literals.insert(label);
                        }
                        rest = tail.strip_prefix('"').expect("closing quote");
                    }
                }
            }
        }
        assert!(literals.len() > 900, "source scan found only {} literals", literals.len());
        let missing: Vec<_> = literals.iter().filter(|label| !has(ru, label)).collect();
        assert!(missing.is_empty(), "untranslated Russian UI literals: {missing:#?}");
    }

    #[test]
    fn russian_history_and_diagnostics_preserve_user_values() {
        let ru = Lang::from_code("ru").expect("ru registered");
        set_current(ru);
        assert_eq!(command_label("Undo Insert pages from Rapport {n}.pdf"), "Отменить Вставить страницы из Rapport {n}.pdf");
        assert_eq!(command_label("Redo Fill in Contact {key}"), "Повторить Заполнить Contact {key}");
        assert_eq!(action_label("Change Title"), "Изменить заголовок");
        assert_eq!(action_label("Custom action {n}"), "Custom action {n}");
        assert_eq!(fmt(t("This page couldn't be displayed.\n{e}"), &[("e", "OS error {n}")]), "Не удалось отобразить эту страницу.\nOS error {n}");
        assert_eq!(fmt(t("{n} pages selected"), &[("n", "3")]), "Выбрано 3 страницы");
        assert_eq!(tr(ru, "CheckBox"), "Флажок");
        set_current(Lang::EN);
    }

    #[test]
    fn bulgarian_is_registered() {
        let bg = Lang::from_code("bg").expect("bg registered");
        assert_eq!(bg.name(), "Български");
        assert_eq!(normalize_pref("BG"), Some("bg"));
        assert_eq!(lang_from_tag("bg_BG.UTF-8"), Some(bg));
        assert_eq!(lang_from_tag("bg-BG"), Some(bg));
        assert_eq!(first_supported("bg-BG\r\nen-US"), Some(bg));
        assert_eq!(tr(bg, "File"), "Файл");
        assert_eq!(tr(bg, "Save as…"), "Запиши като…");
        assert_eq!(tr(bg, "Bookmarks"), "Отметки");
        assert_eq!(tr(bg, "Layers"), "Слоеве");
        assert_eq!(tr(bg, "Rapport de l'utilisateur.pdf"), "Rapport de l'utilisateur.pdf");
        // one (1), other (0, 2+)
        assert_eq!((0..=3).map(|n| (bg.0.plural)(n)).collect::<Vec<_>>(), [1, 0, 1, 1]);
        assert_eq!(trn(bg, 0, "{n} page", "{n} pages"), "0 страници");
        assert_eq!(trn(bg, 1, "{n} page", "{n} pages"), "1 страница");
        assert_eq!(trn(bg, 2, "{n} page", "{n} pages"), "2 страници");
        assert_eq!(trn(bg, 21, "{n} page", "{n} pages"), "21 страници");
        assert_eq!(trn(bg, 1, "{n} field", "{n} fields"), "1 поле");
        assert_eq!(trn(bg, 5, "{n} field", "{n} fields"), "5 полета");
        let mut app = crate::PdfCraftApp::default();
        app.set_option("language", "bg").unwrap();
        assert_eq!(app.language, "bg");
        let mut restored = crate::PdfCraftApp::default();
        restored.restore(&app.persist());
        assert_eq!(restored.language, "bg");
    }

    /// Bulgarian translates every registered command and every All tools group, section and item.
    #[test]
    fn bulgarian_covers_commands_and_catalogue() {
        let bg = Lang::from_code("bg").expect("bg registered");
        for command in pdfcraft_engine::commands::COMMANDS {
            assert!(has(bg, command.label), "missing command: {}", command.label);
            if let Some(menu) = command.menu {
                assert!(has(bg, menu), "missing menu: {menu}");
            }
        }
        for group in pdfcraft_engine::catalog::TOOL_GROUPS {
            assert!(has(bg, group.label), "missing group: {}", group.label);
            for section in group.sections {
                assert!(has(bg, section.title), "missing section: {}", section.title);
                for item in section.items {
                    assert!(has(bg, item.label), "missing item: {}", item.label);
                }
            }
        }
    }

    /// New tl!("literal") labels must not silently fall back to English.
    #[test]
    fn bulgarian_covers_ui_literals() {
        let bg = Lang::from_code("bg").expect("bg registered");
        let mut stack = vec![std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
        let mut literals = std::collections::BTreeSet::new();
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(dir).expect("UI source directory") {
                let path = entry.expect("UI source entry").path();
                if path.is_dir() {
                    if path.file_name().is_some_and(|name| name != "i18n") {
                        stack.push(path);
                    }
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    let source = std::fs::read_to_string(path).expect("UI source file").replace("\r\n", "\n");
                    let mut rest = source.split("#[cfg(test)]\nmod ").next().unwrap_or_default();
                    while let Some((_, after)) = rest.split_once("tl!(\"") {
                        let mut escaped = false;
                        let end = after
                            .char_indices()
                            .find_map(|(i, c)| {
                                if c == '"' && !escaped {
                                    return Some(i);
                                }
                                escaped = c == '\\' && !escaped;
                                None
                            })
                            .expect("closed tl! literal");
                        let (raw, tail) = after.split_at(end);
                        if tail.starts_with("\")") {
                            let label: String = serde_json::from_str(&format!("\"{raw}\"")).expect("UI literal escapes");
                            literals.insert(label);
                        }
                        rest = tail.strip_prefix('"').expect("closing quote");
                    }
                }
            }
        }
        assert!(literals.len() > 900, "source scan found only {} literals", literals.len());
        let missing: Vec<_> = literals.iter().filter(|label| !has(bg, label)).collect();
        assert!(missing.is_empty(), "untranslated Bulgarian UI literals: {missing:#?}");
    }

    #[test]
    fn bulgarian_diagnostics_preserve_user_values() {
        let bg = Lang::from_code("bg").expect("bg registered");
        set_current(bg);
        assert_eq!(action_label("Custom action {n}"), "Custom action {n}");
        assert_eq!(fmt(t("{n} pages selected"), &[("n", "3")]), "Избрани страници: 3");
        assert_eq!(tr(bg, "CheckBox"), "Квадратче за отметка");
        set_current(Lang::EN);
    }

    #[test]
    fn hungarian_is_registered() {
        let hu = Lang::from_code("hu").expect("hu registered");
        assert_eq!(hu.name(), "Magyar");
        assert_eq!(normalize_pref("HU"), Some("hu"));
        assert_eq!(lang_from_tag("hu_HU.UTF-8"), Some(hu));
        assert_eq!(lang_from_tag("hu-HU"), Some(hu));
        assert_eq!(first_supported("hu-HU\r\nen-US"), Some(hu));
        assert_eq!(tr(hu, "File"), "Fájl");
        assert_eq!(tr(hu, "Save as…"), "Mentés másként…");
        assert_eq!(tr(hu, "Bookmarks"), "Könyvjelzők");
        assert_eq!(tr(hu, "Layers"), "Rétegek");
        assert_eq!(tr(hu, "Rapport de l'utilisateur.pdf"), "Rapport de l'utilisateur.pdf");
        assert_eq!(tr_ctx(hu, "signature pad", "Type"), "Gépelés");
        assert_eq!(tr_ctx(hu, "action wizard", "Start"), "Indítás");
        assert_eq!(tr(hu, "Windows store  ·  "), "Windows-tároló  ·  ");
        assert_eq!(tr(hu, "Place saved {what}"), "Mentett {what} elhelyezése");
        assert_eq!(tr(hu, "Remove saved {what}"), "Mentett {what} eltávolítása");
        assert_eq!(tr(hu, "Squiggly"), "Hullámos aláhúzás");
        // one (1), other (0, 2+)
        assert_eq!((0..=3).map(|n| (hu.0.plural)(n)).collect::<Vec<_>>(), [1, 0, 1, 1]);
        assert_eq!(trn(hu, 0, "{n} page", "{n} pages"), "0 oldal");
        assert_eq!(trn(hu, 1, "{n} page", "{n} pages"), "1 oldal");
        assert_eq!(trn(hu, 2, "{n} page", "{n} pages"), "2 oldal");
        assert_eq!(trn(hu, 1, "{n} field", "{n} fields"), "1 mező");
        assert_eq!(trn(hu, 5, "{n} field", "{n} fields"), "5 mező");
        let mut app = crate::PdfCraftApp::default();
        app.set_option("language", "hu").unwrap();
        assert_eq!(app.language, "hu");
        let mut restored = crate::PdfCraftApp::default();
        restored.restore(&app.persist());
        assert_eq!(restored.language, "hu");
    }

    /// Hungarian translates every registered command and every All tools group, section and item.
    #[test]
    fn hungarian_covers_commands_and_catalogue() {
        let hu = Lang::from_code("hu").expect("hu registered");
        for command in pdfcraft_engine::commands::COMMANDS {
            assert!(has(hu, command.label), "missing command: {}", command.label);
            if let Some(menu) = command.menu {
                assert!(has(hu, menu), "missing menu: {menu}");
            }
        }
        for group in pdfcraft_engine::catalog::TOOL_GROUPS {
            assert!(has(hu, group.label), "missing group: {}", group.label);
            for section in group.sections {
                assert!(has(hu, section.title), "missing section: {}", section.title);
                for item in section.items {
                    assert!(has(hu, item.label), "missing item: {}", item.label);
                }
            }
        }
    }

    /// New tl!("literal") labels must not silently fall back to English.
    #[test]
    fn hungarian_covers_ui_literals() {
        let hu = Lang::from_code("hu").expect("hu registered");
        let mut stack = vec![std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
        let mut literals = std::collections::BTreeSet::new();
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(dir).expect("UI source directory") {
                let path = entry.expect("UI source entry").path();
                if path.is_dir() {
                    if path.file_name().is_some_and(|name| name != "i18n") {
                        stack.push(path);
                    }
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    let source = std::fs::read_to_string(path).expect("UI source file").replace("\r\n", "\n");
                    let mut rest = source.split("#[cfg(test)]\nmod ").next().unwrap_or_default();
                    while let Some((_, after)) = rest.split_once("tl!(\"") {
                        let mut escaped = false;
                        let end = after
                            .char_indices()
                            .find_map(|(i, c)| {
                                if c == '"' && !escaped {
                                    return Some(i);
                                }
                                escaped = c == '\\' && !escaped;
                                None
                            })
                            .expect("closed tl! literal");
                        let (raw, tail) = after.split_at(end);
                        if tail.starts_with("\")") {
                            let label: String = serde_json::from_str(&format!("\"{raw}\"")).expect("UI literal escapes");
                            literals.insert(label);
                        }
                        rest = tail.strip_prefix('"').expect("closing quote");
                    }
                }
            }
        }
        assert!(literals.len() > 900, "source scan found only {} literals", literals.len());
        let missing: Vec<_> = literals.iter().filter(|label| !has(hu, label)).collect();
        assert!(missing.is_empty(), "untranslated Hungarian UI literals: {missing:#?}");
    }

    #[test]
    fn hungarian_history_and_diagnostics_preserve_user_values() {
        let hu = Lang::from_code("hu").expect("hu registered");
        set_current(hu);
        assert_eq!(command_label("Undo Insert pages from Rapport {n}.pdf"), "Oldalak beszúrása ebből: Rapport {n}.pdf visszavonása");
        assert_eq!(command_label("Redo Fill in Contact {key}"), "Contact {key} kitöltése megismétlése");
        assert_eq!(action_label("Change Title"), "Cím módosítása");
        assert_eq!(action_label("Custom action {n}"), "Custom action {n}");
        assert_eq!(
            fmt(t("This page couldn't be displayed.\n{e}"), &[("e", "OS error {n}")]),
            "Ezt az oldalt nem sikerült megjeleníteni.\nOS error {n}"
        );
        assert_eq!(fmt(t("{n} pages selected"), &[("n", "3")]), "3 oldal kijelölve");
        assert_eq!(tr(hu, "CheckBox"), "Jelölőnégyzet");
        set_current(Lang::EN);
    }

    #[test]
    fn ukrainian_is_registered_and_persists() {
        let uk = Lang::from_code("uk").expect("uk registered");
        assert_eq!(uk.name(), "Українська");
        assert_eq!(normalize_pref("UK"), Some("uk"));
        for tag in ["uk", "uk-UA", "uk_UA.UTF-8", "UK-ua", "uk_UA@variant"] {
            assert_eq!(lang_from_tag(tag), Some(uk), "{tag}");
        }
        assert_eq!(first_supported("uk-UA\r\nen-US"), Some(uk));
        assert_eq!(first_supported("(\n    \"uk-UA\",\n    \"en-US\"\n)\n"), Some(uk));
        assert_eq!(tr(uk, "File"), "Файл");
        assert_eq!(tr(uk, "Save as…"), "Зберегти як…");
        assert_eq!(tr(uk, "Bookmarks"), "Закладки");
        assert_eq!(tr(uk, "Layers"), "Шари");
        assert_eq!(tr(uk, "Звіт {n}.pdf"), "Звіт {n}.pdf");
        assert_eq!(tr_ctx(uk, "comment menu", "Edit"), "Редагувати");
        assert_eq!(tr_ctx(uk, "signature pad", "Type"), "Ввести");
        assert_eq!(tr_ctx(uk, "action wizard", "Start"), "Почати");
        assert_eq!(tr(uk, "Subject"), "Тема");
        assert_eq!(tr_ctx(uk, "certificate", "Subject"), "Власник сертифіката");
        let mut app = crate::PdfCraftApp::default();
        app.set_option("language", "UK").unwrap();
        assert_eq!(app.language, "uk");
        let mut restored = crate::PdfCraftApp::default();
        restored.restore(&app.persist());
        assert_eq!(restored.language, "uk");
        assert!(restored.set_option("language", "xx").is_err());
        assert_eq!(restored.language, "uk");
    }

    #[test]
    fn ukrainian_integer_plurals_cover_teens_and_large_counts() {
        let uk = Lang::from_code("uk").expect("uk registered");
        for (counts, page, field) in [
            (vec![1, 21, 31, 101, 121, 1001], "сторінка", "поле"),
            (vec![2, 3, 4, 22, 23, 24, 102, 104, 122, 1002], "сторінки", "поля"),
            (vec![0, 5, 10, 11, 12, 13, 14, 19, 20, 25, 100, 110, 111, 112, 114, 1000, u64::MAX], "сторінок", "полів"),
        ] {
            for n in counts {
                assert_eq!(trn(uk, n, "{n} page", "{n} pages"), format!("{n} {page}"));
                assert_eq!(trn(uk, n, "{n} field", "{n} fields"), format!("{n} {field}"));
            }
        }
    }

    #[test]
    fn ukrainian_calendar_uses_month_case_for_dates() {
        let uk = Lang::from_code("uk").expect("uk registered");
        for (source, standalone, date) in [
            ("January", "Січень", "січня"),
            ("February", "Лютий", "лютого"),
            ("March", "Березень", "березня"),
            ("April", "Квітень", "квітня"),
            ("May", "Травень", "травня"),
            ("June", "Червень", "червня"),
            ("July", "Липень", "липня"),
            ("August", "Серпень", "серпня"),
            ("September", "Вересень", "вересня"),
            ("October", "Жовтень", "жовтня"),
            ("November", "Листопад", "листопада"),
            ("December", "Грудень", "грудня"),
        ] {
            assert_eq!(tr(uk, source), standalone);
            let month = tr_ctx(uk, "calendar date", source);
            assert_eq!(month, date);
            assert_eq!(fmt(tr(uk, "{month} {day}, {y}"), &[("month", month), ("day", "9"), ("y", "2026")]), format!("9 {date} 2026"));
            // Catalogs without this context retain their existing month translation.
            assert_eq!(tr_ctx(Lang::EN, "calendar date", source), source);
            assert_eq!(tr_ctx(JA(), "calendar date", source), tr(JA(), source));
        }
    }

    /// Preserve only product/format names, abbreviations and templates with no English words.
    #[test]
    fn ukrainian_catalog_does_not_leave_english_ui_text() {
        let uk = Lang::from_code("uk").expect("uk registered");
        let (entries, errors) = parse_entries(uk.0.source, uk.0.plural_forms());
        assert!(errors.is_empty(), "{errors:?}");
        let keep = [
            "OK",
            "PDF/A…",
            "Microsoft Word (.docx)",
            "PostScript / EPS",
            // Key-cap names, printed in English on Ukrainian keyboards.
            "Home / End",
            "JavaScript",
            "ZIP",
            "{n} {kind}",
            "{rule} - {status}",
            "{field}: {e}",
            "`{command}` {when}",
            "{0}: {1}",
            "{month} {y}",
            "+LOC",
            "−LOC",
            "ΔLOC",
            "+Bin",
            "−Bin",
        ];
        let unchanged: Vec<_> = entries
            .iter()
            .filter(|e| {
                let key_cap = e.context == "key" && KEY_CAP_NAMES.contains(&e.source.as_str());
                e.source == e.translation && !keep.contains(&e.source.as_str()) && !key_cap
            })
            .collect();
        assert!(unchanged.is_empty(), "untranslated Ukrainian catalog entries: {unchanged:#?}");
    }

    /// Ukrainian translates every registered command and every All tools label.
    #[test]
    fn ukrainian_covers_commands_and_catalogue() {
        let uk = Lang::from_code("uk").expect("uk registered");
        for command in pdfcraft_engine::commands::COMMANDS {
            assert!(has(uk, command.label), "missing command: {}", command.label);
            if let Some(menu) = command.menu {
                assert!(has(uk, menu), "missing menu: {menu}");
            }
        }
        for group in pdfcraft_engine::catalog::TOOL_GROUPS {
            assert!(has(uk, group.label), "missing group: {}", group.label);
            for section in group.sections {
                assert!(has(uk, section.title), "missing section: {}", section.title);
                for item in section.items {
                    assert!(has(uk, item.label), "missing item: {}", item.label);
                }
            }
        }
    }

    /// Check direct lookups and multiline literals as well as ordinary tl!("literal") calls,
    /// following the complete-catalog check for Simplified Chinese.
    #[test]
    fn ukrainian_covers_ui_literals() {
        let uk = Lang::from_code("uk").expect("uk registered");
        let mut stack = vec![std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
        let mut literals = std::collections::BTreeSet::new();
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(dir).expect("UI source directory") {
                let path = entry.expect("UI source entry").path();
                if path.is_dir() {
                    if path.file_name().is_some_and(|name| name != "i18n") {
                        stack.push(path);
                    }
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    let source = std::fs::read_to_string(path).expect("UI source file").replace("\r\n", "\n").replace("crate::i18n::t(", "tl!(");
                    let mut rest = source.split("#[cfg(test)]\nmod ").next().unwrap_or_default();
                    while let Some((_, after)) = rest.split_once("tl!(") {
                        let after = after.trim_start();
                        let Some(after) = after.strip_prefix('"') else {
                            rest = after;
                            continue;
                        };
                        let mut escaped = false;
                        let end = after
                            .char_indices()
                            .find_map(|(i, c)| {
                                if c == '"' && !escaped {
                                    return Some(i);
                                }
                                escaped = c == '\\' && !escaped;
                                None
                            })
                            .expect("closed tl! literal");
                        let (raw, tail) = after.split_at(end);
                        let closing = tail.strip_prefix('"').expect("closing quote").trim_start();
                        let closing = closing.strip_prefix(',').unwrap_or(closing).trim_start();
                        if closing.starts_with(')') {
                            let label: String = serde_json::from_str(&format!("\"{raw}\"")).expect("UI literal escapes");
                            literals.insert(label);
                        }
                        rest = tail.strip_prefix('"').expect("closing quote");
                    }
                }
            }
        }
        assert!(literals.len() > 900, "source scan found only {} literals", literals.len());
        let missing: Vec<_> = literals.iter().filter(|label| !has(uk, label)).collect();
        assert!(missing.is_empty(), "untranslated Ukrainian UI literals: {missing:#?}");
    }

    #[test]
    fn ukrainian_history_and_diagnostics_preserve_user_values() {
        let uk = Lang::from_code("uk").expect("uk registered");
        set_current(uk);
        assert_eq!(command_label("Undo Insert pages from Звіт {n}.pdf"), "Скасувати: Вставити сторінки з Звіт {n}.pdf");
        assert_eq!(command_label("Redo Fill in Контакт {key}"), "Повторити: Заповнити Контакт {key}");
        assert_eq!(action_label("Change Title"), "Змінити: Заголовок");
        assert_eq!(action_label("Custom action {n}"), "Custom action {n}");
        assert_eq!(fmt(t("This page couldn't be displayed.\n{e}"), &[("e", "OS error {n}")]), "Не вдалося відобразити цю сторінку.\nOS error {n}");
        assert_eq!(fmt(t("{n} pages selected"), &[("n", "3")]), "Вибрано сторінок: 3");
        assert_eq!(fmt(t("{n} contributors · {c} commits"), &[("n", "2"), ("c", "1,234")]), "Учасників: 2 · Комітів: 1,234");
        assert_eq!(tr(uk, "CheckBox"), "Прапорець");
        let contributor = crate::credits::Contributor {
            login: "reader{n}",
            display_name: Some("Save"),
            real_name: None,
            prs: 2,
            commits: 3,
            lines_added: 10,
            lines_deleted: 4,
            binary_added: 1,
            binary_deleted: 0,
            first_commit: "2026-10-01T00:00:00Z",
            last_commit: "2026-10-09T00:00:00Z",
        };
        assert_eq!(
            contributor.summary(),
            "@reader{n}: PR: 2, комітів: 3, +10 / −4 рядків (Δ +6), +1 / −0 бінарних ресурсів, 2026-10-01 – 2026-10-09"
        );
        assert_eq!(contributor.name(crate::credits::NameMode::DisplayName), "Save");
        set_current(Lang::EN);
    }

    fn arabic() -> Lang {
        Lang::from_code("ar").expect("ar registered")
    }

    #[test]
    fn arabic_is_registered_and_persists() {
        let ar = arabic();
        assert_eq!(ar.name(), "العربية");
        assert!(ar.rtl());
        assert!(Lang::all().filter(|l| l.rtl()).eq([ar]), "Arabic is the only right-to-left language");
        assert_eq!(normalize_pref("AR"), Some("ar"));
        for tag in ["ar", "ar-MA", "ar_EG.UTF-8", "AR-sa", "ar_DZ@variant"] {
            assert_eq!(lang_from_tag(tag), Some(ar), "{tag}");
        }
        assert_eq!(first_supported("ar-MA\r\nen-US"), Some(ar));
        assert_eq!(tr(ar, "File"), "ملف");
        assert_eq!(tr(ar, "Bookmarks"), "المرجعية الإشارات");
        assert_eq!(tr_ctx(ar, "comment menu", "Edit"), "تحرير");
        assert_eq!(tr_ctx(ar, "certificate", "Subject"), "الشهادة صاحب");
        // Unknown strings and user text are never reordered by a lookup.
        assert_eq!(tr(ar, "تقرير نهائي {n}.pdf"), "تقرير نهائي {n}.pdf");
        let mut app = crate::PdfCraftApp::default();
        app.set_option("language", "AR").unwrap();
        assert_eq!(app.language, "ar");
        let mut restored = crate::PdfCraftApp::default();
        restored.restore(&app.persist());
        assert_eq!(restored.language, "ar");
        assert!(restored.set_option("language", "xx").is_err());
        assert_eq!(restored.language, "ar");
    }

    /// Lookups return Arabic in display order: egui draws the words left to right as given, and
    /// joins the letters of each word itself.
    #[test]
    fn arabic_lookups_are_in_display_order() {
        let ar = arabic();
        // "حفظ باسم…" as typed; the ellipsis ends the phrase on the left.
        assert_eq!(tr(ar, "Save as…"), "…باسم حفظ");
        // Latin names keep their own direction inside the phrase.
        assert_eq!(tr(ar, "About PdfCraft"), "PdfCraft حول");
        // A caption's colon stays beside the widget on its right.
        assert_eq!(tr(ar, "Name:"), "الاسم:");
        assert_eq!(tr(ar, "Issued by: "), "عن صادرة: ");
        // Placeholders survive and are filled in place.
        assert_eq!(tr(ar, "Couldn't open {name}: {e}"), "{name}: {e} فتح تعذّر");
        assert_eq!(fmt(tr(ar, "Couldn't open {name}: {e}"), &[("name", "a {e}.pdf"), ("e", "x")]), "a {e}.pdf: x فتح تعذّر");
        assert_eq!(fmt(tr(ar, "Page {p} of {n}"), &[("p", "3"), ("n", "12")]), "12 من 3 الصفحة");
        set_current(ar);
        assert_eq!(command_label("Undo Insert pages from a.pdf"), "a.pdf من صفحات إدراج عن التراجع");
        assert_eq!(menu_label("file.saveAs", "Save as…"), "…باسم حفظ");
        set_current(Lang::EN);
    }

    #[test]
    fn arabic_plurals_follow_the_six_cldr_forms() {
        let ar = arabic();
        assert_eq!(ar.0.plural_forms(), 6);
        let form = |n| (ar.0.plural)(n);
        assert_eq!([0, 1, 2].map(form), [0, 1, 2]);
        assert_eq!([3, 10, 103, 110, 1003].map(form), [3; 5]);
        assert_eq!([11, 99, 111, 199, 1011].map(form), [4; 5]);
        assert_eq!([100, 101, 102, 200, 1000, u64::MAX - 13].map(form), [5; 6]);
        assert_eq!(trn(ar, 1, "{n} page", "{n} pages"), "صفحة 1");
        assert_eq!(trn(ar, 2, "{n} page", "{n} pages"), "صفحتان 2");
        assert_eq!(trn(ar, 5, "{n} page", "{n} pages"), "صفحات 5");
        assert_eq!(trn(ar, 11, "{n} field", "{n} fields"), "حقلًا 11");
        assert_eq!(trn(ar, 100, "{n} field", "{n} fields"), "حقل 100");
    }

    /// Arabic translates every registered command, menu and All tools label.
    #[test]
    fn arabic_covers_commands_and_catalogue() {
        let ar = arabic();
        for command in pdfcraft_engine::commands::COMMANDS {
            assert!(has(ar, command.label), "missing command: {}", command.label);
            if let Some(menu) = command.menu {
                assert!(has(ar, menu), "missing menu: {menu}");
            }
        }
        for group in pdfcraft_engine::catalog::TOOL_GROUPS {
            assert!(has(ar, group.label), "missing group: {}", group.label);
            for section in group.sections {
                assert!(has(ar, section.title), "missing section: {}", section.title);
                for item in section.items {
                    assert!(has(ar, item.label), "missing item: {}", item.label);
                }
            }
        }
    }

    /// Arabic translates every `tl!("…")` literal in the UI source.
    #[test]
    fn arabic_covers_ui_literals() {
        let literals = ui_literals();
        assert!(literals.len() > 900, "source scan found only {} literals", literals.len());
        let missing: Vec<_> = literals.iter().filter(|label| !has(arabic(), label)).collect();
        assert!(missing.is_empty(), "untranslated Arabic UI literals: {missing:#?}");
    }

    /// With a craft-fonts Arabic face, every Arabic translation has glyphs: with all interface
    /// faces (desktop) and with the Arabic face as the only craft-fonts face (the web build keeps
    /// it; Arabic labels must not need a Japanese face).
    #[test]
    fn arabic_labels_have_glyphs() {
        let faces: Vec<String> = pdfcraft_fonts::ui_arabic_fonts().iter().map(|f| f.name()).collect();
        if faces.is_empty() {
            eprintln!("skipping Arabic glyph checks: build with CRAFT_FONTS_DIR and an Arabic face to run them");
            return;
        }
        let ar = arabic();
        let (entries, _) = parse_entries(ar.0.source, ar.0.plural_forms());
        let labels: String = entries.iter().flat_map(|e| e.translation.chars()).chain(ar.name().chars()).filter(|c| !c.is_control()).collect();
        let mut arabic_only = crate::theme::font_definitions();
        for family in arabic_only.families.values_mut() {
            family.retain(|name| !pdfcraft_fonts::CRAFT_FONTS.iter().any(|face| face.name() == *name) || faces.contains(name));
        }
        use egui::epaint::text::{Fonts, TextOptions};
        for (build, defs) in [("all faces", crate::theme::font_definitions()), ("Arabic face only", arabic_only)] {
            let mut fonts = Fonts::new(TextOptions::default(), defs);
            for id in [egui::FontId::proportional(13.0), egui::FontId::monospace(13.0), crate::theme::medium(13.0), crate::theme::semibold(17.0)] {
                let missing: std::collections::BTreeSet<char> = labels.chars().filter(|c| !fonts.has_glyphs(&id, &c.to_string())).collect();
                assert!(missing.is_empty(), "{build}: {id:?} lacks Arabic label glyphs {missing:?}");
            }
        }
    }

    /// Every bundled catalog is well-formed and consistent with its sources.
    #[test]
    fn bundled_catalogs_are_consistent() {
        let mut codes = std::collections::HashSet::new();
        for l in &LANGUAGES {
            assert!(codes.insert(l.code), "duplicate code {}", l.code);
            assert!(l.code == l.code.to_ascii_lowercase() && !l.name.is_empty(), "{}", l.code);
            let (entries, errors) = parse_entries(l.source, l.plural_forms());
            assert!(errors.is_empty(), "{}: {errors:?}", l.code);
            for e in entries.iter().filter(|e| e.context == "@id") {
                let command = pdfcraft_engine::commands::command(&e.source);
                let Some(command) = command else { panic!("{}: unknown command id {:?}", l.code, e.source) };
                assert_eq!(placeholders(&e.translation), placeholders(command.label), "{}: {}", l.code, e.source);
                assert_eq!(e.translation.ends_with('…'), command.label.ends_with('…'), "{}: ellipsis mismatch: {}", l.code, e.source);
            }
        }
    }

    /// Preferences ▸ Date format ▸ Language offers exactly the interface languages, in order.
    #[test]
    fn date_languages_are_the_interface_languages() {
        let interface: Vec<(&str, &str)> = Lang::all().map(|l| (l.code(), l.name())).collect();
        let dates: Vec<(&str, &str)> = pdfcraft_engine::dates::DATE_LANGUAGES.iter().map(|l| (l.code, l.name)).collect();
        assert_eq!(dates, interface);
    }
}
