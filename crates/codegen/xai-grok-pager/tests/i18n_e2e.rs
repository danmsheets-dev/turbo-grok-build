//! Live-apply proof for the i18n runtime path (English-only product).
//!
//! Linked WITHOUT `cfg(test)` so `i18n::apply` really calls
//! `rust_i18n::set_locale`.

use xai_grok_pager::i18n;

#[test]
fn apply_english_only() {
    i18n::apply(Some("en"));
    assert_eq!(i18n::tr_or("welcome.quit", "Quit"), "Quit");

    // Former multi-locale requests still resolve English.
    i18n::apply(Some("zh-CN"));
    assert_eq!(i18n::tr_or("welcome.quit", "Quit"), "Quit");
    i18n::apply(Some("ja"));
    assert_eq!(i18n::tr_or("welcome.quit", "Quit"), "Quit");
    i18n::apply(Some("auto"));
    assert_eq!(i18n::tr_or("welcome.quit", "Quit"), "Quit");

    assert_eq!(i18n::resolve_locale(Some("zh-Hant")), "en");
    assert_eq!(i18n::resolve_locale(Some("pt")), "en");
    for loc in i18n::SUPPORTED_LOCALES {
        assert_eq!(&i18n::resolve_locale(Some(loc)), loc);
    }
    assert_eq!(i18n::SUPPORTED_LOCALES, &["en"]);
}
