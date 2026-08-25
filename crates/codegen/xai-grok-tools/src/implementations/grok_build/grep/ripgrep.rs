use std::path::PathBuf;
use std::sync::OnceLock;

#[cfg(bundle_rg)]
const RG_BYTES: &[u8] = include_bytes!(concat!(
    env!("OUT_DIR"),
    "/bundle-rg/rg-",
    env!("GROK_TOOLS_RG_VER"),
    "-",
    env!("GROK_TOOLS_RG_TARGET"),
    ".bin"
));

/// File name for the extracted bundled ripgrep binary.
///
/// On Windows the binary is a PE image and **must** carry a `.exe` suffix so
/// `CreateProcess` / `Command::new` reliably treat it as executable. The old
/// extensionless name (`rg-<ver>-<target>`) is still accepted if already on
/// disk so existing installs keep working without a re-extract.
#[cfg(bundle_rg)]
fn bundled_rg_file_name() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        concat!(
            "rg-",
            env!("GROK_TOOLS_RG_VER"),
            "-",
            env!("GROK_TOOLS_RG_TARGET"),
            ".exe"
        )
    }
    #[cfg(not(target_os = "windows"))]
    {
        concat!(
            "rg-",
            env!("GROK_TOOLS_RG_VER"),
            "-",
            env!("GROK_TOOLS_RG_TARGET")
        )
    }
}

#[cfg(bundle_rg)]
fn resolve_bundled_rg() -> std::io::Result<PathBuf> {
    #[cfg(target_os = "windows")]
    use std::fs;
    let vendor = crate::util::grok_home().join("vendor");
    let p = vendor.join(bundled_rg_file_name());
    // Windows: older installs left an extensionless PE (`rg-<ver>-<target>`).
    // `CreateProcess` / `Command::new` does **not** reliably treat that as
    // executable — spawn fails in ~2 ms with empty model-facing stdout (the
    // error landed only in stderr). Promote the legacy file to the `.exe`
    // name; never return the bare path as the spawn target. If promotion
    // fails (read-only sandbox), fall through to re-extract from the bundle.
    #[cfg(target_os = "windows")]
    {
        let legacy = vendor.join(concat!(
            "rg-",
            env!("GROK_TOOLS_RG_VER"),
            "-",
            env!("GROK_TOOLS_RG_TARGET")
        ));
        if !p.exists() && legacy.exists() {
            if let Err(e) = fs::create_dir_all(p.parent().unwrap_or(vendor.as_path())) {
                tracing::warn!(error = %e, "grep: could not create vendor dir for rg.exe promote");
            } else if let Err(e) = fs::copy(&legacy, &p) {
                tracing::warn!(
                    error = %e,
                    legacy = %legacy.display(),
                    target = %p.display(),
                    "grep: could not promote extensionless rg to .exe; will re-extract"
                );
            }
            // Prefer the promoted/extracted `.exe` path below; do not return
            // `legacy` even when copy failed — that path is known-broken for
            // spawn on Windows.
        }
    }
    write_bundled_rg_if_needed(&p)?;
    Ok(p)
}

#[cfg(bundle_rg)]
fn rg_bytes_sha256() -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(RG_BYTES).into()
}

#[cfg(bundle_rg)]
fn file_sha256(path: &std::path::Path) -> Option<[u8; 32]> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(path).ok()?;
    Some(Sha256::digest(&bytes).into())
}

/// Write the bundled ripgrep bytes when the vendor file is missing or does
/// not match the compile-time SHA-256. A pre-planted `~/.grok/vendor/rg-*`
/// must not be executed.
#[cfg(bundle_rg)]
fn write_bundled_rg_if_needed(p: &std::path::Path) -> std::io::Result<()> {
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    let matches = p.is_file() && file_sha256(p).as_ref() == Some(&rg_bytes_sha256());
    if !matches {
        if p.exists() {
            tracing::warn!(
                path = %p.display(),
                "grep: vendor ripgrep hash mismatch; rewriting from the embedded bundle"
            );
        }
        fs::create_dir_all(p.parent().unwrap())?;
        fs::write(p, RG_BYTES)?;
        #[cfg(unix)]
        {
            let mut perms = fs::metadata(p)?.permissions();
            perms.set_mode(0o755);
            fs::set_permissions(p, perms)?;
        }
    }
    Ok(())
}

/// On Windows, if `path` is an existing extensionless PE (legacy vendor extract),
/// copy it next to itself as `path.exe` and return that. `CreateProcess` does
/// not reliably run extensionless PE images; returning the bare path is how
/// grep short-circuited empty on Windows field reports.
///
/// **Do not use `Path::with_extension`**: for names like `rg-15.0.0-override`
/// the "extension" is the tail after the last `.` (`0-override`), so
/// `with_extension("exe")` would yield the wrong `rg-15.0.exe`.
fn ensure_windows_exe_suffix(path: PathBuf) -> PathBuf {
    #[cfg(not(windows))]
    {
        return path;
    }
    #[cfg(windows)]
    {
        use std::ffi::OsStr;
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with(".exe"))
        {
            return path;
        }
        if !path.exists() {
            return path;
        }
        // Append ".exe" to the full file name (not with_extension).
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            return path;
        };
        let exe_name = format!("{name}.exe");
        let exe = path.with_file_name(OsStr::new(&exe_name));
        if exe.exists() {
            return exe;
        }
        if std::fs::copy(&path, &exe).is_ok() {
            return exe;
        }
        // Last resort: return the original; spawn will surface a clear error.
        path
    }
}

/// Exact vendor filenames we will execute — never a `rg-*` prefix scan.
/// A pre-planted `~/.grok/vendor/rg-evil` must not be picked up.
fn expected_vendor_rg_names() -> Vec<String> {
    let ver = option_env!("GROK_TOOLS_RG_VER").unwrap_or("15.0.0");
    let target = option_env!("GROK_TOOLS_RG_TARGET").unwrap_or("unknown");
    let mut names = vec![
        format!("rg-{ver}-{target}"),
        format!("rg-{ver}-{target}.exe"),
        format!("rg-{ver}-override"),
        format!("rg-{ver}-override.exe"),
        "rg.exe".into(),
        "rg".into(),
    ];
    names.sort();
    names.dedup();
    names
}

fn find_vendor_rg() -> Option<PathBuf> {
    let vendor = crate::util::grok_home().join("vendor");
    #[cfg(bundle_rg)]
    {
        let p = vendor.join(bundled_rg_file_name());
        if p.is_file() {
            return Some(p);
        }
    }
    for name in expected_vendor_rg_names() {
        let p = vendor.join(name);
        if p.is_file() {
            return Some(ensure_windows_exe_suffix(p));
        }
    }
    None
}

/// Get the path to the ripgrep executable.
///
/// In release builds with bundling enabled, this extracts the bundled ripgrep
/// binary to ~/.grok/vendor/ and returns that path.
/// Otherwise, assumes `rg` is in PATH.
pub fn rg_path() -> PathBuf {
    static RG_EXEC: OnceLock<PathBuf> = OnceLock::new();
    RG_EXEC
        .get_or_init(|| {
            #[cfg(bundle_rg)]
            {
                resolve_bundled_rg()
                    .map(ensure_windows_exe_suffix)
                    .unwrap_or_else(|_| find_vendor_rg().unwrap_or_else(|| PathBuf::from("rg")))
            }
            #[cfg(not(bundle_rg))]
            {
                // RG_BIN_PATH: explicit override (tests / packaging can set this).
                if let Ok(p) = std::env::var("RG_BIN_PATH") {
                    return ensure_windows_exe_suffix(PathBuf::from(p));
                }
                // Prefer a previously-extracted vendor binary (same home as
                // release builds). Without this, non-bundled debug builds on
                // Windows fall through to bare `rg` which is often not on PATH
                // — the grep tool then Early-returns in ~2 ms with an empty
                // model body (spawn program-not-found).
                if let Some(vendor) = find_vendor_rg() {
                    return vendor;
                }
                // Some hermetic test runners set RUNFILES_DIR and ship rg as a
                // data dependency rather than on PATH. Scan for a directory
                // entry containing "ripgrep_hermetic" and prefer arch-scoped
                // paths when present.
                if let Ok(rf) = std::env::var("RUNFILES_DIR") {
                    let base = PathBuf::from(rf);
                    if let Ok(entries) = std::fs::read_dir(&base) {
                        for entry in entries.flatten() {
                            let name = entry.file_name();
                            if name.to_string_lossy().contains("ripgrep_hermetic") {
                                for sub in ["amd64/rg", "arm64/rg", "rg"] {
                                    let candidate = entry.path().join(sub);
                                    if candidate.exists() {
                                        return ensure_windows_exe_suffix(candidate);
                                    }
                                }
                            }
                        }
                    }
                }
                PathBuf::from("rg")
            }
        })
        .clone()
}
