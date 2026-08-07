//! Shared test utilities for minimal mode.
//!
//! Compiled only in `#[cfg(test)]` builds. Import via `crate::test_util`.
//!
//! Deliberately absent from `guard.rs`'s `include_str!` inventory: that guard
//! scans the crate's *shipping* modules for forbidden resize helpers, and this
//! module never reaches a release build.

/// Pin this crate's unit-test binary to the modern (non-legacy) glyph set.
///
/// Every glyph minimal mode paints comes from `xai_grok_pager_render::glyphs`,
/// which picks its variant at runtime via `is_legacy_windows_console()`
/// (crates/codegen/xai-grok-pager-render/src/glyphs.rs:522). That probe
/// default-denies to "legacy ConHost" on Windows whenever the terminal-brand
/// probe returns `Unknown`, and under `cargo test` no terminal env var is set —
/// so a Windows test host is misclassified and every fixture silently gets the
/// ASCII/BMP substitutions (`●` → `•`, and the low-color theme ladder).
///
/// `xai-grok-pager-render` pins the same variable for *its own* test binary
/// (its `test_util.rs`), but that `#[ctor]` lives in a `#[cfg(test)]` module of
/// that crate; this crate links pager-render as a plain dependency, without
/// `cfg(test)`, so nothing here inherits it. Hence the second copy.
///
/// On non-Windows hosts `decide_legacy_windows_console` returns early for
/// `host != Windows`, so this is a no-op there and nothing any test asserts
/// changes. The legacy substitution keeps its own direct coverage in
/// `pager-render`'s `glyphs.rs`.
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
/// `abs_path("test/session")` yields `/test/session` on POSIX and
/// `C:\test\session` on Windows.
///
/// A bare POSIX literal is *not* absolute on Windows: `Path::is_absolute()`
/// requires a `Prefix` component (a drive letter), so `/test/session` has a
/// root but no prefix and reports `false`. The session-cwd elision in
/// `xai_grok_pager_render` compares two paths that must both be absolute for
/// the prefix strip to fire, so a POSIX literal quietly disables it and the
/// fixture asserts against an un-elided path.
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

/// Rewrite `/` to the host path separator in an **expected** display string.
///
/// Paths painted by the render crate keep their native spelling — tool headers
/// run the path through `xai_grok_paths::normalize_lexically`, which rebuilds
/// the path from `Path::components()` and therefore emits `\` on Windows
/// (crates/common/xai-grok-paths/src/lib.rs). An expectation of
/// `"src/main.rs"` is a POSIX-only spelling of the same path.
pub fn native_sep(path: &str) -> String {
    if cfg!(windows) {
        path.replace('/', "\\")
    } else {
        path.to_string()
    }
}
