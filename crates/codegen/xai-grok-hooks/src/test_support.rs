//! Test-only helpers shared across `xai-grok-hooks` unit + integration tests.
//!
//! This module is gated on `#[cfg(test)]` and is exported as `pub(crate)`
//! so any in-crate `#[cfg(test)] mod tests` can use it. Integration tests
//! under `tests/` cannot reach it; for those, copy or re-implement the
//! handful of functions here that they need (the only one currently used
//! by integration tests is unrelated).

use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};

/// Run `f` with the env var `name` set to `value` (or unset if `value`
/// is `None`), restoring the previous value on return.
///
/// Uses `catch_unwind` so a panic inside `f` does not leak the env var
/// into the rest of the test process.
///
/// `cargo test` runs tests in parallel by default. Process env vars are
/// process-global, so callers should pick uniquely-named vars to avoid
/// inter-test races. The lifecycle here (save -> set -> run -> restore)
/// is panic-safe but not race-safe.
///
/// **FOLLOW-UP**: the helper does not
/// enforce the unique-name discipline -- a future contributor passing
/// a common name like `HOME` could trigger flaky tests. The standard
/// fix is to add `serial_test` as a dev-dep and decorate every
/// env-touching test with `#[serial(env_var)]` so the test runner
/// serialises them. For now the unique-name
/// convention plus `catch_unwind` restoration is sufficient for the
/// tests that ship today.
pub(crate) fn with_env_var<R>(name: &str, value: Option<&str>, f: impl FnOnce() -> R) -> R {
    let previous = std::env::var_os(name);
    // SAFETY: env-var writes are not thread-safe. Callers use uniquely
    // named vars so no concurrent test races on the same name.
    unsafe {
        match value {
            Some(v) => std::env::set_var(name, v),
            None => std::env::remove_var(name),
        }
    }

    let result = catch_unwind(AssertUnwindSafe(f));

    // SAFETY: see above. Restore unconditionally so a panic doesn't
    // leak env state to subsequent tests.
    unsafe {
        match previous {
            Some(prev) => std::env::set_var(name, prev),
            None => std::env::remove_var(name),
        }
    }

    match result {
        Ok(value) => value,
        Err(payload) => resume_unwind(payload),
    }
}

// =============================================================================
// Portable hook-command builders
// =============================================================================
//
// `run_command_hook` routes any command containing a shell metacharacter
// through a shell: literally `sh -c` on unix, and whatever
// `xai_grok_config::shell::detect_windows_shell()` picked on Windows —
// PowerShell 7, PowerShell 5.1, Git Bash, or `cmd` (shell.rs:30). A fixture
// that hardcodes POSIX syntax therefore only tests the unix arm; on a default
// Windows host `echo 'x' >&2` is a PowerShell parse error and the hook exits 1
// instead of doing what the test meant.
//
// These builders emit the same *behaviour* in whichever shell the product will
// actually use, so the assertion stays about the runner and not about the host.

/// Which shell family `run_command_hook` will hand a shell command to on this
/// host. Read from the product's own detector, so the fixture and the runner
/// can never disagree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HookShell {
    /// `sh -c` (unix) or Git Bash (`bash -c`) on Windows: POSIX syntax.
    Posix,
    /// `powershell.exe` / `pwsh` with `-Command`.
    PowerShell,
    /// `cmd /C`.
    Cmd,
}

/// The shell family the command branch of [`crate::runner::command::run_command_hook`]
/// will use on this host.
pub(crate) fn hook_shell() -> HookShell {
    #[cfg(unix)]
    {
        HookShell::Posix
    }
    #[cfg(not(unix))]
    {
        use xai_grok_config::shell::WindowsShell;
        match xai_grok_config::shell::detect_windows_shell() {
            WindowsShell::GitBash(_) => HookShell::Posix,
            WindowsShell::Pwsh | WindowsShell::PowerShell => HookShell::PowerShell,
            WindowsShell::Cmd => HookShell::Cmd,
        }
    }
}

/// A command that writes `message` to **stderr** and then exits with `code`.
///
/// Used by the gate/stop tests, where stderr is the block reason and the exit
/// code is the block signal.
///
/// `message` must not contain a single quote (every arm quotes with `'`), and
/// `cmd`'s arm additionally cannot carry `&`, `|`, `<`, `>` or `^`.
pub(crate) fn stderr_then_exit(message: &str, code: i32) -> String {
    debug_assert!(
        !message.contains('\''),
        "stderr_then_exit cannot quote a single quote: {message:?}"
    );
    match hook_shell() {
        HookShell::Posix => format!("echo '{message}' >&2; exit {code}"),
        // PowerShell has no `1>&2`: streams can only be merged *into* stream 1.
        // `[Console]::Error` is the direct handle and needs no redirection.
        HookShell::PowerShell => {
            format!("[Console]::Error.WriteLine('{message}'); exit {code}")
        }
        HookShell::Cmd => format!("(echo {message})1>&2 & exit /b {code}"),
    }
}

/// A command that prints `text` verbatim on stdout.
///
/// `text` is emitted as a single-quoted literal on the POSIX and PowerShell
/// arms, so it must not contain a single quote.
pub(crate) fn echo_stdout(text: &str) -> String {
    debug_assert!(
        !text.contains('\''),
        "echo_stdout cannot quote a single quote: {text:?}"
    );
    match hook_shell() {
        HookShell::Posix => format!("echo '{text}'"),
        HookShell::PowerShell => format!("Write-Output '{text}'"),
        HookShell::Cmd => format!("echo {text}"),
    }
}

/// Write a hook script into `dir` whose body is "exit 0 iff the environment
/// variable `var` equals `expected`", and return the **file name** to invoke.
///
/// The extension and body match [`hook_shell`]: a `#!/bin/sh` script on the
/// POSIX arm, a `.cmd` batch file otherwise (PowerShell can invoke a `.cmd`,
/// but neither PowerShell nor `cmd` can execute a `#!`-script).
pub(crate) fn write_env_check_script(dir: &std::path::Path, var: &str, expected: &str) -> String {
    match hook_shell() {
        HookShell::Posix => {
            let name = "hook.sh";
            let path = dir.join(name);
            std::fs::write(
                &path,
                format!("#!/bin/sh\ntest \"${{{var}}}\" = \"{expected}\"\n"),
            )
            .unwrap();
            make_executable(&path);
            name.to_string()
        }
        HookShell::PowerShell | HookShell::Cmd => {
            let name = "hook.cmd";
            let path = dir.join(name);
            // `if not "%VAR%"=="expected" exit /b 1` — quoting both sides keeps
            // an unset variable from breaking the `if` grammar.
            std::fs::write(
                &path,
                format!(
                    "@echo off\r\nif not \"%{var}%\"==\"{expected}\" exit /b 1\r\nexit /b 0\r\n"
                ),
            )
            .unwrap();
            name.to_string()
        }
    }
}

/// Write a hook script into `dir` that exits 0 unconditionally, returning the
/// **file name** to invoke. See [`write_env_check_script`] for why the
/// extension is host-dependent.
pub(crate) fn write_exit0_script(dir: &std::path::Path) -> String {
    match hook_shell() {
        HookShell::Posix => {
            let name = "hook.sh";
            let path = dir.join(name);
            std::fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
            make_executable(&path);
            name.to_string()
        }
        HookShell::PowerShell | HookShell::Cmd => {
            let name = "hook.cmd";
            std::fs::write(dir.join(name), "@echo off\r\nexit /b 0\r\n").unwrap();
            name.to_string()
        }
    }
}

/// Build a command string that runs `file` out of the directory named by the
/// environment variable `var`, keeping the `${VAR}` reference intact so the
/// caller still exercises the runner's `$` → shell routing and its
/// env-substitution step.
///
/// PowerShell needs the call operator (`&`) plus quoting, because a bare path
/// token is only treated as a command when it is unquoted *and* space-free —
/// and a temp directory may well contain a space.
pub(crate) fn invoke_script_via_env(var: &str, file: &str) -> String {
    match hook_shell() {
        HookShell::Posix => format!("\"${{{var}}}/{file}\""),
        HookShell::PowerShell => format!("& \"${{{var}}}\\{file}\""),
        HookShell::Cmd => format!("call \"${{{var}}}\\{file}\""),
    }
}

/// `chmod +x` on unix; a no-op elsewhere (Windows has no exec bit).
fn make_executable(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restores_previous_value_on_normal_return() {
        let key = "GROK_HOOKS_TEST_SUPPORT_RESTORE";
        with_env_var(key, Some("first"), || {
            with_env_var(key, Some("second"), || {
                assert_eq!(std::env::var(key).unwrap(), "second");
            });
            assert_eq!(std::env::var(key).unwrap(), "first");
        });
        assert!(std::env::var(key).is_err());
    }

    #[test]
    fn restores_previous_unset_state_on_normal_return() {
        let key = "GROK_HOOKS_TEST_SUPPORT_UNSET_RESTORE";
        // SAFETY: see module-level note.
        unsafe {
            std::env::remove_var(key);
        }
        with_env_var(key, Some("temporary"), || {
            assert_eq!(std::env::var(key).unwrap(), "temporary");
        });
        assert!(std::env::var(key).is_err());
    }

    #[test]
    fn restores_after_panic() {
        let key = "GROK_HOOKS_TEST_SUPPORT_PANIC_RESTORE";
        // SAFETY: see module-level note.
        unsafe {
            std::env::remove_var(key);
        }
        let panicked = catch_unwind(AssertUnwindSafe(|| {
            with_env_var(key, Some("during-panic"), || {
                panic!("intentional");
            });
        }));
        assert!(panicked.is_err(), "expected panic to propagate");
        assert!(
            std::env::var(key).is_err(),
            "env var must be restored after panic"
        );
    }

    #[test]
    fn allows_explicit_unset() {
        let key = "GROK_HOOKS_TEST_SUPPORT_EXPLICIT_UNSET";
        // SAFETY: see module-level note.
        unsafe {
            std::env::set_var(key, "before");
        }
        with_env_var(key, None, || {
            assert!(std::env::var(key).is_err());
        });
        assert_eq!(std::env::var(key).unwrap(), "before");
        // SAFETY: see module-level note.
        unsafe {
            std::env::remove_var(key);
        }
    }
}
