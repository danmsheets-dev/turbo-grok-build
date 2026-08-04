//! Path-segment guards for untrusted identifiers.
//!
//! Turbo is the subagent-heavy fork: `spawn_many` fan-out, worktree isolation
//! on by default, background tasks and per-agent memory all take an id that
//! originated with the model (or with a peer agent) and join it into a
//! filesystem path — `~/.grok/worktrees/<slug>/<label>`,
//! `{sessions_cwd}/{session_id}/subagents/<id>/meta.json`, and friends. That is
//! more id→path surface than upstream has, so the validation lives here, in the
//! shared type crate, rather than being re-derived (differently, and wrongly) at
//! each call site.
//!
//! Two design rules govern every helper below:
//!
//! 1. **Fail closed, never sanitise.** These are predicates, not normalisers.
//!    A caller that "cleaned up" a bad id would map two *distinct* ids onto one
//!    directory — a subagent could then read or overwrite another subagent's
//!    worktree metadata simply by picking a name that sanitises the same way.
//!    Rejecting is the only safe answer.
//! 2. **Allowlist, not denylist.** We accept `[A-Za-z0-9_-]` (plus `.` for task
//!    ids) and reject everything else. A denylist of "bad characters" has to be
//!    right about every encoding trick; an allowlist only has to be right about
//!    what real ids look like.
//!
//! ## These are Windows-correctness rules, not only security rules
//!
//! Turbo is Windows-primary, and several of the checks below have nothing to do
//! with an attacker:
//!
//! - `CON`, `NUL`, `COM1`… are *device* names on Win32. `File::create("nul")`
//!   silently succeeds and writes to the bit bucket, so a subagent literally
//!   named `nul` would appear to work and then lose every byte of its metadata.
//! - Win32 strips a trailing `.` or space from a path component before it
//!   reaches the filesystem, so `foo.` and `foo` are the *same* directory. Two
//!   ids that differ only by a trailing dot would silently collide.
//! - `:` starts an NTFS alternate data stream (`id:hidden`) and also forms a
//!   drive prefix (`C:`), either of which escapes the intended parent.
//! - `\` is a path separator on Windows even though it is an ordinary character
//!   on Unix, and `\\?\` / `\\server\share` prefixes bypass path normalisation
//!   entirely. Rejecting `\` unconditionally keeps a Linux-developed id from
//!   becoming a traversal only once it runs on Windows.
//!
//! ## Known limitation: case folding
//!
//! NTFS is case-insensitive by default, so `Task-A` and `task-a` name the same
//! directory on Windows and different directories on Linux. These predicates
//! deliberately do **not** force a case, because rejecting mixed case would
//! break existing ids. Callers must therefore not treat "two ids passed the
//! guard" as "two ids address different directories" on Windows — use the
//! existing canonicalise-before-match confinement checks for that. The guards
//! here stop *escape*, not case-folded aliasing.

/// Maximum length of a single guarded path segment, in bytes.
///
/// 128 is chosen against the tightest real constraint rather than the loosest:
/// a legacy Win32 path (no `\\?\` prefix) caps out at 260 characters *total*,
/// and the deepest path we build from one of these ids is roughly
/// `C:\Users\<user>\.grok\sessions\<percent-encoded-cwd>\<session-id>\subagents\<id>\meta.json`.
/// The percent-encoded cwd alone can run past 100 characters, so two 128-byte
/// segments plus the fixed prefix already sits near the limit; anything larger
/// would trade a theoretical id length for `ERROR_PATH_NOT_FOUND` at runtime.
/// It is also comfortably under the NTFS/ext4 per-component limit of 255 (all
/// accepted characters are ASCII, so bytes == UTF-16 code units here).
pub const MAX_SAFE_PATH_SEGMENT_LEN: usize = 128;

/// Win32 reserved device names. Reserved regardless of extension and regardless
/// of case: `CON`, `con`, `Con.txt` and `CON.log` all resolve to the console
/// device, not to a file.
///
/// `COM0`/`LPT0` are not reserved; only 1-9 are (the superscript variants
/// `COM¹`/`COM²`/`COM³` are also reserved on modern Windows but cannot reach us
/// — the allowlist rejects all non-ASCII).
const WINDOWS_RESERVED_DEVICE_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Whether `stem` is a Win32 reserved device name (case-insensitive).
fn is_windows_reserved_device_name(stem: &str) -> bool {
    // Device names are pure ASCII, so ASCII-uppercasing is exactly the folding
    // Win32 applies; `to_uppercase` (full Unicode) would be both slower and
    // wrong for e.g. Turkish dotless-i locales.
    WINDOWS_RESERVED_DEVICE_NAMES
        .iter()
        .any(|reserved| stem.eq_ignore_ascii_case(reserved))
}

/// Shared body for the guards below.
///
/// `allow_dot` widens the allowlist to include `.` for identifier flavours that
/// historically carry one (`task.v1`, MCP-style `server.tool` call ids). The
/// traversal, device-name and Windows-stripping rules apply either way, so
/// `allow_dot` can never turn `..` or `foo.` into an accepted value.
fn is_safe_segment_inner(s: &str, allow_dot: bool) -> bool {
    // Empty: `Path::join("")` is a no-op, so an empty id would silently address
    // the *parent* directory instead of a child of it.
    if s.is_empty() || s.len() > MAX_SAFE_PATH_SEGMENT_LEN {
        return false;
    }

    // Explicit traversal. `..` is also caught by the allowlist when `allow_dot`
    // is false, but naming it here keeps the intent readable and covers the
    // task-id flavour.
    if s == "." || s == ".." {
        return false;
    }

    // Win32 strips a trailing dot or space from a path component, so `foo.`,
    // `foo ` and `foo` are one directory. Reject rather than normalise: two ids
    // must never resolve to the same path. (A leading space is rejected by the
    // allowlist below, which never accepts a space anywhere.)
    if s.ends_with('.') || s.ends_with(' ') {
        return false;
    }

    // The allowlist. This single pass is what rejects `/`, `\` (Windows
    // separator and `\\?\` / UNC prefix lead-in), `:` (drive prefix `C:` and
    // NTFS alternate data streams), NUL, every other C0/C1 control character,
    // whitespace, wildcards, and all non-ASCII — including Unicode forms that
    // would normalise into a separator on macOS or fold together on NTFS.
    if !s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || (allow_dot && c == '.'))
    {
        return false;
    }

    // Device-name check runs on the stem before the first dot, because Win32
    // resolves `nul.txt` to the NUL device just as it resolves `nul`.
    let stem = s.split('.').next().unwrap_or(s);
    if is_windows_reserved_device_name(stem) {
        return false;
    }

    true
}

/// Whether `s` may be used as a single filesystem path segment.
///
/// The strictest flavour: `[A-Za-z0-9_-]` only, no dots at all. Use it for
/// segments we generate and fully control the shape of — worktree labels,
/// session directory components, slugs.
#[inline]
pub fn is_safe_path_segment(s: &str) -> bool {
    is_safe_segment_inner(s, false)
}

/// Whether `s` is safe to join as a task / subagent id.
///
/// Same escape and Windows rules as [`is_safe_path_segment`], but internal dots
/// are permitted so historical id shapes (`task.v1`, `server.tool`) keep
/// working. `.`, `..` and a trailing `.` are still rejected.
#[inline]
pub fn is_safe_task_id(s: &str) -> bool {
    is_safe_segment_inner(s, true)
}

/// Whether `s` is safe to use as a subagent / agent name.
///
/// Agent names become directory components under per-agent memory and worktree
/// roots, and they are also matched against configured agent definitions, so
/// they get the strict no-dot allowlist: an agent named `..` or `general.` must
/// never resolve, and allowing a dot would let `explore.` and `explore` name the
/// same directory on Windows.
#[inline]
pub fn is_safe_agent_name(s: &str) -> bool {
    is_safe_segment_inner(s, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_realistic_ids() {
        for good in [
            // UUIDv7-style subagent id, the common case.
            "subagent-019e5f2c-7a10-7c31-9b44-2f0d61a8e3bb",
            "019e5f2c7a107c319b442f0d61a8e3bb",
            "general-purpose",
            "explore",
            "task-1",
            "parent_session",
            "a",
        ] {
            assert!(is_safe_path_segment(good), "{good:?} should be accepted");
            assert!(is_safe_task_id(good), "{good:?} should be accepted");
            assert!(is_safe_agent_name(good), "{good:?} should be accepted");
        }

        // Exactly at the cap is still accepted; one over is not (see
        // `rejects_over_length_ids`).
        let at_cap = "x".repeat(MAX_SAFE_PATH_SEGMENT_LEN);
        assert!(is_safe_path_segment(&at_cap));
        assert!(is_safe_task_id(&at_cap));
        assert!(is_safe_agent_name(&at_cap));
    }

    #[test]
    fn task_ids_allow_internal_dots_but_segments_and_agent_names_do_not() {
        assert!(is_safe_task_id("task.v1"));
        assert!(is_safe_task_id("gateway.search"));
        assert!(!is_safe_path_segment("task.v1"));
        assert!(!is_safe_agent_name("task.v1"));
        // Hyphenated equivalents are fine everywhere.
        assert!(is_safe_path_segment("task-v1"));
        assert!(is_safe_agent_name("task-v1"));
    }

    #[test]
    fn rejects_empty() {
        assert!(!is_safe_path_segment(""));
        assert!(!is_safe_task_id(""));
        assert!(!is_safe_agent_name(""));
    }

    #[test]
    fn rejects_dot_and_dotdot() {
        for bad in [".", ".."] {
            assert!(!is_safe_path_segment(bad), "{bad:?} should be rejected");
            assert!(!is_safe_task_id(bad), "{bad:?} should be rejected");
            assert!(!is_safe_agent_name(bad), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn rejects_path_separators_and_traversal() {
        for bad in [
            "a/b",
            "a\\b",
            "../etc/passwd",
            "..\\windows\\system32",
            "a/../b",
            "/absolute",
            "\\absolute",
            "trailing/",
            "trailing\\",
        ] {
            assert!(!is_safe_path_segment(bad), "{bad:?} should be rejected");
            assert!(!is_safe_task_id(bad), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn rejects_drive_prefixes_unc_and_alternate_data_streams() {
        for bad in [
            "C:",
            "C:\\Windows",
            "c:temp",
            // A `\\?\` or UNC prefix cannot survive the backslash rejection.
            "\\\\?\\C:\\Windows",
            "\\\\server\\share",
            // NTFS alternate data stream.
            "id:hidden",
            "id:$DATA",
        ] {
            assert!(!is_safe_path_segment(bad), "{bad:?} should be rejected");
            assert!(!is_safe_task_id(bad), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn rejects_nul_and_control_characters() {
        for bad in [
            "null\0byte",
            "\0",
            "bell\x07",
            "esc\x1b[0m",
            "newline\nid",
            "carriage\rreturn",
            "tab\tid",
            "del\x7f",
        ] {
            assert!(!is_safe_path_segment(bad), "{bad:?} should be rejected");
            assert!(!is_safe_task_id(bad), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn rejects_windows_reserved_device_names_any_case_with_or_without_extension() {
        for bad in [
            "CON", "con", "Con", "PRN", "prn", "AUX", "aux", "NUL", "nul", "COM1", "com9", "LPT1",
            "lpt9",
        ] {
            assert!(!is_safe_path_segment(bad), "{bad:?} should be rejected");
            assert!(!is_safe_agent_name(bad), "{bad:?} should be rejected");
            assert!(!is_safe_task_id(bad), "{bad:?} should be rejected");
        }
        // With an extension the stem still names the device.
        for bad in ["con.txt", "NUL.log", "com1.json", "aux.tar.gz"] {
            assert!(!is_safe_task_id(bad), "{bad:?} should be rejected");
        }
        // Names that merely start with a device prefix are fine.
        for good in ["console", "com10", "lpt0", "com0", "nulled", "auxiliary"] {
            assert!(is_safe_path_segment(good), "{good:?} should be accepted");
            assert!(is_safe_task_id(good), "{good:?} should be accepted");
        }
    }

    #[test]
    fn rejects_trailing_dot_or_space_because_windows_strips_them() {
        for bad in ["foo.", "foo ", "foo..", "foo. ", "foo .", " foo", "a b"] {
            assert!(!is_safe_path_segment(bad), "{bad:?} should be rejected");
            assert!(!is_safe_task_id(bad), "{bad:?} should be rejected");
        }
        // The collision this prevents: `foo.` and `foo` are one directory on
        // Win32, so exactly one of the pair may be accepted.
        assert!(is_safe_task_id("foo"));
        assert!(!is_safe_task_id("foo."));
    }

    #[test]
    fn rejects_over_length_ids() {
        let too_long = "x".repeat(MAX_SAFE_PATH_SEGMENT_LEN + 1);
        assert!(!is_safe_path_segment(&too_long));
        assert!(!is_safe_task_id(&too_long));
        assert!(!is_safe_agent_name(&too_long));
    }

    #[test]
    fn rejects_non_ascii_and_shell_or_glob_metacharacters() {
        for bad in [
            // Non-ASCII: normalisation and case folding differ per platform.
            "café",
            "агент",
            "id\u{200b}zero-width",
            // Fullwidth solidus normalises to `/` in some contexts.
            "a\u{ff0f}b",
            // Metacharacters that would leak into globs, shells or URLs.
            "id*", "id?", "id|x", "id<y", "id>z", "id\"q", "a;b", "a&b", "a$b", "id%2f",
        ] {
            assert!(!is_safe_path_segment(bad), "{bad:?} should be rejected");
            assert!(!is_safe_task_id(bad), "{bad:?} should be rejected");
        }
    }
}
