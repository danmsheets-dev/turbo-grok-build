//! Shared test utilities for the render crate.
//!
//! Compiled only in `#[cfg(test)]` builds. Import via `crate::test_util`.

/// Pin this crate's unit-test binary to the modern (non-legacy) glyph set.
///
/// The glyph every helper in [`crate::glyphs`] returns is chosen at runtime by
/// `is_legacy_windows_console()` (crates/codegen/xai-grok-pager-render/src/glyphs.rs:522),
/// which default-denies to "legacy ConHost" on Windows whenever the
/// terminal-brand probe returns `Unknown` — and under `cargo test` no terminal
/// env var is set, so a Windows test host is misclassified as a legacy console.
/// The probe is right about a bare `cmd.exe` window; it is the *test binary's*
/// environment that is unrepresentative.
///
/// On non-Windows hosts the probe is a compile-time `false`
/// (`decide_legacy_windows_console` returns early for `host != Windows`), so
/// these tests have only ever exercised the modern glyph set. Pinning it here
/// removes the host dependence without changing what any test asserts; the
/// legacy substitution keeps its own direct coverage in `glyphs.rs`
/// (`decide_legacy_windows_console`, `to_legacy_glyphs`, and the
/// `*_are_one_column` width tests, which assert both variants explicitly).
///
/// `GROK_FORCE_LEGACY_CONSOLE` is the escape hatch the probe already documents
/// (`parse_forced_legacy_console`, glyphs.rs:543); `=0` forces it off. This
/// must run pre-`main`, because the probe caches its answer in a `OnceLock` on
/// first read and libtest gives no ordering guarantee between the test threads
/// that read it.
#[ctor::ctor]
fn pin_modern_console_glyphs_for_tests() {
    // SAFETY: `#[ctor]` runs before `main`, so no other thread exists yet.
    unsafe { std::env::set_var("GROK_FORCE_LEGACY_CONSOLE", "0") };
}

/// Build a host-native **absolute** fixture path from `/`-separated segments.
///
/// `abs_path("Users/me/project/src/main.rs")` yields
/// `/Users/me/project/src/main.rs` on POSIX and
/// `C:\Users\me\project\src\main.rs` on Windows.
///
/// A bare POSIX literal is *not* absolute on Windows: `Path::is_absolute()`
/// requires a `Prefix` component (a drive letter), so `/Users/me/x.rs` has a
/// root but no prefix and reports `false`. Every "absolute path" consumer in
/// this crate then takes its not-absolute arm silently — `file_path_to_url`
/// hands the path to `url::Url::from_file_path`, which refuses it and yields
/// `osc8_url: None` (render/osc8.rs:263), and
/// `file_link_presentation_for_resolved` classifies the painted text as
/// relative (render/osc8.rs:286). Fixtures that mean "an absolute path on this
/// host" must go through here.
pub fn abs_path(segments: &str) -> String {
    let segments = segments.trim_start_matches('/');
    if cfg!(windows) {
        format!(r"C:\{}", segments.replace('/', "\\"))
    } else {
        format!("/{segments}")
    }
}

/// [`abs_path`] as a `PathBuf`.
pub fn abs_path_buf(segments: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(abs_path(segments))
}

/// `file://` URL for [`abs_path`]'s spelling of `segments`.
///
/// `url::Url::from_file_path` keeps the drive letter and forward slashes on
/// Windows, so the same fixture is `file:///a/b` on POSIX and
/// `file:///C:/a/b` on Windows. `segments` is inserted verbatim, so callers
/// pass already-percent-encoded text (`a%20b.rs`).
pub fn abs_file_url(segments: &str) -> String {
    let segments = segments.trim_start_matches('/');
    if cfg!(windows) {
        format!("file:///C:/{segments}")
    } else {
        format!("file:///{segments}")
    }
}

/// Rewrite `/` to the host path separator in an **expected** display string.
///
/// Paths painted by this crate keep their native spelling — tool headers run
/// the path through `xai_grok_paths::normalize_lexically`, which rebuilds the
/// path from `Path::components()` and therefore emits `\` on Windows
/// (crates/common/xai-grok-paths/src/lib.rs). An expectation of
/// `"src/main.rs"` is a POSIX-only spelling of the same path.
pub fn native_sep(path: &str) -> String {
    if cfg!(windows) {
        path.replace('/', "\\")
    } else {
        path.to_string()
    }
}
