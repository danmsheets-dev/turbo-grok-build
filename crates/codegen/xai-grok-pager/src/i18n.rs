//! UI language (i18n) — English-only for Turbo Grok Build.
//!
//! Translations live in `locales/en.yml` and are embedded at compile time by
//! `rust-i18n` (loaded once in `lib.rs`). Non-English UI locales are not
//! supported; config values other than `en` are coerced to English.
//!
//! **Scope policy**: TUI strings use the English bundle. Headless / ACP /
//! doctor output is also English-only (machine-consumed surfaces).

/// Canonical UI-language choices for the `language` setting.
/// Keep in sync with `settings/defs.rs::LANGUAGE_CHOICES` and `locales/*.yml`.
pub const SUPPORTED_LOCALES: &[&str] = &["en"];

/// Canonicalize a raw `[ui].language` value.
///
/// Turbo is English-only: anything other than an explicit `en` is treated as
/// `en` (including legacy `auto` and former multi-locale values).
pub fn canonical_language(value: Option<&str>) -> &'static str {
    let _ = value;
    "en"
}

/// Resolve the effective locale id from the configured language.
///
/// Always English for this product line.
pub fn resolve_locale(configured: Option<&str>) -> &'static str {
    let _ = configured;
    "en"
}

/// Set the process-wide UI locale from the configured language.
/// Cheap and idempotent — safe to call on every settings commit.
///
/// Under `cfg(test)` this is a no-op: `rust_i18n::set_locale` flips a
/// process-global atomic, and lib unit tests run multi-threaded in one
/// process.
pub fn apply(configured: Option<&str>) {
    #[cfg(not(test))]
    rust_i18n::set_locale(resolve_locale(configured));
    #[cfg(test)]
    let _ = configured;
}

/// Initialize the UI locale at startup, before the first render.
pub fn init(configured: Option<&str>) {
    apply(configured);
}

/// Localized "press {key} again to {label}" pending-confirmation hint,
/// shared by the full TUI and the minimal pager — sibling crates can't
/// invoke `t!` themselves (the macro only exists inside this crate).
pub fn press_again_hint(key: &str, label: &str) -> String {
    rust_i18n::t!("shortcuts.press_again_key", key = key, label = label).into_owned()
}

/// Translate a runtime-computed key for the current locale, falling back to
/// `fallback` (the English source text) when the key has no bundle entry.
pub fn tr_or<'a>(key: &str, fallback: &'a str) -> std::borrow::Cow<'a, str> {
    let locale = rust_i18n::locale();
    crate::_rust_i18n_try_translate(locale.as_ref(), key)
        .map(|c| std::borrow::Cow::Owned(c.into_owned()))
        .unwrap_or(std::borrow::Cow::Borrowed(fallback))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tr_or_translates_dynamic_keys_and_falls_back() {
        let value = tr_or("welcome.quit", "Quit");
        assert_eq!(value, "Quit");
        let missing = tr_or("settings.__definitely_missing__.label", "English source");
        assert_eq!(missing, "English source");
    }

    #[test]
    fn english_only_canonical() {
        assert_eq!(canonical_language(None), "en");
        assert_eq!(canonical_language(Some("")), "en");
        assert_eq!(canonical_language(Some("en")), "en");
        assert_eq!(canonical_language(Some("auto")), "en");
        assert_eq!(canonical_language(Some("zh-CN")), "en");
        assert_eq!(canonical_language(Some("de")), "en");
        assert_eq!(resolve_locale(Some("ja")), "en");
        assert_eq!(SUPPORTED_LOCALES, &["en"]);
    }
}
