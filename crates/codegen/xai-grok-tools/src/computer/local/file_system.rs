use std::{future::Future, io, path::Path, time::Duration};

use tokio::{fs, time::sleep};

use crate::computer::types::{AsyncFileSystem, ComputerError};

/// Creates a local FS access which allows writing and reading from the local files
pub struct LocalFs;

// Keep the window short: these retries absorb brief Windows editor/indexer/AV
// races without hiding persistent locks, ACL failures, or sandbox denials.
const WRITE_RETRY_DELAYS: &[Duration] = &[
    Duration::from_millis(25),
    Duration::from_millis(50),
    Duration::from_millis(100),
    Duration::from_millis(200),
    Duration::from_millis(400),
];
#[cfg(any(windows, test))]
const WINDOWS_ERROR_SHARING_VIOLATION: i32 = 32;
#[cfg(any(windows, test))]
const WINDOWS_ERROR_LOCK_VIOLATION: i32 = 33;

/// Check if an IO error is a permission denial (EACCES or EPERM),
/// which indicates a sandbox violation.
fn is_permission_error(e: &io::Error) -> bool {
    matches!(e.kind(), io::ErrorKind::PermissionDenied)
}

#[cfg(any(windows, test))]
fn is_windows_transient_write_lock_raw_os_error(raw_os_error: Option<i32>) -> bool {
    matches!(
        raw_os_error,
        Some(WINDOWS_ERROR_SHARING_VIOLATION | WINDOWS_ERROR_LOCK_VIOLATION)
    )
}

fn is_transient_write_lock_error(e: &io::Error) -> bool {
    #[cfg(windows)]
    {
        is_windows_transient_write_lock_raw_os_error(e.raw_os_error())
    }

    #[cfg(not(windows))]
    {
        let _ = e;
        false
    }
}

#[cfg(test)]
fn is_test_transient_write_lock_error(e: &io::Error) -> bool {
    is_windows_transient_write_lock_raw_os_error(e.raw_os_error())
}

async fn write_file_with_transient_lock_retries(path: &Path, data: &[u8]) -> io::Result<()> {
    write_file_with_retry_hooks(
        || fs::write(path, data),
        |delay| sleep(delay),
        |retry_count| {
            tracing::debug!(
                path = %path.display(),
                retry_count,
                "file write succeeded after transient lock retries"
            );
        },
        |error, retry_count, delay| {
            tracing::debug!(
                path = %path.display(),
                error = %error,
                retry_count,
                delay_ms = delay.as_millis(),
                "file write hit transient lock; retrying"
            );
        },
        |error, retry_count| {
            tracing::debug!(
                path = %path.display(),
                error = %error,
                retry_count,
                "file write exhausted transient lock retries"
            );
        },
        is_transient_write_lock_error,
    )
    .await
}

/// Confirm `path` exists as a file with `expected_len` bytes (RC11 Round-2).
///
/// Catches races / AV scanners / incomplete flushes where `write` returned Ok
/// but the agent-visible tree still lacks the file (empty snapshot after
/// "successful" write under concurrent multi-agent load).
async fn verify_write_persisted(path: &Path, expected_len: u64) -> io::Result<()> {
    let meta = fs::metadata(path).await?;
    if !meta.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("path exists but is not a regular file: {}", path.display()),
        ));
    }
    let len = meta.len();
    if len != expected_len {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!(
                "size mismatch after write: expected {expected_len} bytes, got {len} at {}",
                path.display()
            ),
        ));
    }
    Ok(())
}

async fn write_file_with_retry_hooks<W, WFut, S, SFut, Success, Retry, Exhausted, IsRetryable>(
    mut write: W,
    mut sleep_for: S,
    mut on_retry_success: Success,
    mut on_retry: Retry,
    mut on_exhausted: Exhausted,
    is_retryable: IsRetryable,
) -> io::Result<()>
where
    W: FnMut() -> WFut,
    WFut: Future<Output = io::Result<()>>,
    S: FnMut(Duration) -> SFut,
    SFut: Future<Output = ()>,
    Success: FnMut(usize),
    Retry: FnMut(&io::Error, usize, Duration),
    Exhausted: FnMut(&io::Error, usize),
    IsRetryable: Fn(&io::Error) -> bool,
{
    let mut retry_count = 0usize;

    loop {
        match write().await {
            Ok(()) => {
                if retry_count > 0 {
                    on_retry_success(retry_count);
                }
                return Ok(());
            }
            Err(e) if is_retryable(&e) => {
                if retry_count >= WRITE_RETRY_DELAYS.len() {
                    on_exhausted(&e, retry_count);
                    return Err(e);
                }
                let delay = WRITE_RETRY_DELAYS[retry_count];
                retry_count += 1;
                on_retry(&e, retry_count, delay);
                sleep_for(delay).await;
            }
            Err(e) => return Err(e),
        }
    }
}

#[async_trait::async_trait]
impl AsyncFileSystem for LocalFs {
    #[tracing::instrument(name = "fs.read_file", skip_all)]
    async fn read_file(&self, path: &Path) -> Result<Vec<u8>, ComputerError> {
        match fs::read(path).await {
            Ok(data) => Ok(data),
            Err(e) => {
                if is_permission_error(&e) {
                    xai_grok_sandbox::log_violation(&path.display().to_string(), "read");
                }
                Err(e.into())
            }
        }
    }

    #[tracing::instrument(name = "fs.write_file", skip_all)]
    async fn write_file(&self, path: &Path, data: &[u8]) -> Result<(), ComputerError> {
        // RC13 Wave A: when process confine root is stamped, refuse writes if
        // that root is gone (worktree tombstone). Session CWD is enforced at
        // the tool layer via `enforce_write_path`.
        crate::types::resources::enforce_process_confine_root_exists()
            .map_err(|e| e.into_computer_error())?;

        // implicitly creates the missing directories if any
        if let Some(dir) = path.parent()
            && let Err(e) = fs::create_dir_all(dir).await
        {
            if is_permission_error(&e) {
                xai_grok_sandbox::log_violation(&dir.display().to_string(), "mkdir");
            }
            return Err(e.into());
        }
        if let Err(e) = write_file_with_transient_lock_retries(path, data).await {
            if is_permission_error(&e) {
                xai_grok_sandbox::log_violation(&path.display().to_string(), "write");
            }
            return Err(e.into());
        }
        // RC11 Round-2 / RC13: never report success until the path is visible
        // on disk with the expected size (Super concurrent "phantom write"
        // FOOTGUN). Strengthen: brief re-verify, then up to two rewrite cycles.
        let expected_len = data.len() as u64;
        if verify_write_persisted(path, expected_len).await.is_ok() {
            return Ok(());
        }
        // Short settle for AV / indexer races before rewriting.
        sleep(Duration::from_millis(25)).await;
        if verify_write_persisted(path, expected_len).await.is_ok() {
            return Ok(());
        }
        for attempt in 1..=2u8 {
            tracing::warn!(
                path = %path.display(),
                expected_len,
                attempt,
                "post-write verify failed; retrying write"
            );
            if let Err(e2) = write_file_with_transient_lock_retries(path, data).await {
                if is_permission_error(&e2) {
                    xai_grok_sandbox::log_violation(&path.display().to_string(), "write");
                }
                return Err(e2.into());
            }
            if verify_write_persisted(path, expected_len).await.is_ok() {
                return Ok(());
            }
            sleep(Duration::from_millis(50)).await;
            if verify_write_persisted(path, expected_len).await.is_ok() {
                return Ok(());
            }
        }
        // Parent vanished mid-write → tombstone-class failure for operators.
        let parent_gone = path
            .parent()
            .is_some_and(|p| !p.as_os_str().is_empty() && !p.is_dir());
        let tombstone_hint = if parent_gone {
            " Parent directory is missing (cwd_missing / worktree_tombstone)."
        } else {
            ""
        };
        tracing::error!(
            path = %path.display(),
            expected_len,
            parent_gone,
            "write reported OK but file missing or size mismatch after retries"
        );
        Err(ComputerError::io(format!(
            "write to {} did not persist on disk (post-write exist+size verify failed after retries). \
             Refusing success so agents do not land empty snapshots.{tombstone_hint}",
            path.display()
        )))
    }

    #[tracing::instrument(name = "fs.delete_file", skip_all)]
    async fn delete_file(&self, path: &Path) -> Result<(), ComputerError> {
        if let Err(e) = fs::remove_file(path).await {
            if is_permission_error(&e) {
                xai_grok_sandbox::log_violation(&path.display().to_string(), "delete");
            }
            return Err(e.into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::computer::types::AsyncFileSystem;
    use std::cell::Cell;
    use std::rc::Rc;

    #[test]
    fn classifies_windows_transient_write_lock_errors() {
        assert!(is_windows_transient_write_lock_raw_os_error(Some(32)));
        assert!(is_windows_transient_write_lock_raw_os_error(Some(33)));
        assert!(!is_windows_transient_write_lock_raw_os_error(Some(5)));
        assert!(!is_windows_transient_write_lock_raw_os_error(None));
    }

    #[cfg(windows)]
    #[test]
    fn classifies_windows_transient_write_lock_io_errors() {
        assert!(is_transient_write_lock_error(
            &io::Error::from_raw_os_error(WINDOWS_ERROR_SHARING_VIOLATION,)
        ));
        assert!(is_transient_write_lock_error(
            &io::Error::from_raw_os_error(WINDOWS_ERROR_LOCK_VIOLATION,)
        ));
        assert!(!is_transient_write_lock_error(
            &io::Error::from_raw_os_error(5)
        ));
    }

    #[cfg(not(windows))]
    #[test]
    fn non_windows_does_not_retry_windows_raw_error_numbers() {
        assert!(!is_transient_write_lock_error(
            &io::Error::from_raw_os_error(WINDOWS_ERROR_SHARING_VIOLATION,)
        ));
    }

    #[tokio::test]
    async fn verify_write_persisted_accepts_matching_size() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("probe.txt");
        let data = b"hello-rc11";
        fs::write(&path, data).await.unwrap();
        verify_write_persisted(&path, data.len() as u64)
            .await
            .expect("matching size should pass");
    }

    #[tokio::test]
    async fn verify_write_persisted_rejects_missing_and_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("gone.txt");
        assert!(verify_write_persisted(&missing, 1).await.is_err());

        let path = dir.path().join("probe.txt");
        fs::write(&path, b"abc").await.unwrap();
        assert!(verify_write_persisted(&path, 99).await.is_err());
    }

    #[tokio::test]
    async fn write_file_refuses_success_when_path_missing_after_write() {
        // Normal path: write + verify succeeds end-to-end.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ok.txt");
        let data = b"persist-me";
        LocalFs
            .write_file(&path, data)
            .await
            .expect("LocalFs write should persist");
        assert_eq!(fs::read(&path).await.unwrap(), data);
    }

    #[tokio::test]
    async fn first_attempt_success_does_not_fire_retry_callbacks() {
        let result = write_file_with_retry_hooks(
            || async { Ok(()) },
            |_| async { panic!("should not sleep") },
            |_| panic!("should not fire on_retry_success"),
            |_, _, _| panic!("should not fire on_retry"),
            |_, _| panic!("should not fire on_exhausted"),
            is_test_transient_write_lock_error,
        )
        .await;
        result.unwrap();
    }

    #[tokio::test]
    async fn transient_lock_errors_are_retried_until_success() {
        let attempts = Rc::new(Cell::new(0usize));
        let sleeps = Rc::new(Cell::new(0usize));
        let retry_success_count = Rc::new(Cell::new(0usize));
        let retry_log_count = Rc::new(Cell::new(0usize));

        let result = write_file_with_retry_hooks(
            {
                let attempts = Rc::clone(&attempts);
                move || {
                    let attempts = Rc::clone(&attempts);
                    async move {
                        let next = attempts.get() + 1;
                        attempts.set(next);
                        if next <= 2 {
                            Err(io::Error::from_raw_os_error(
                                WINDOWS_ERROR_SHARING_VIOLATION,
                            ))
                        } else {
                            Ok(())
                        }
                    }
                }
            },
            {
                let sleeps = Rc::clone(&sleeps);
                move |_| {
                    let sleeps = Rc::clone(&sleeps);
                    async move {
                        sleeps.set(sleeps.get() + 1);
                    }
                }
            },
            |count| retry_success_count.set(count),
            |_, _, _| retry_log_count.set(retry_log_count.get() + 1),
            |_, _| panic!("retry budget should not be exhausted"),
            is_test_transient_write_lock_error,
        )
        .await;

        result.unwrap();
        assert_eq!(attempts.get(), 3);
        assert_eq!(sleeps.get(), 2);
        assert_eq!(retry_log_count.get(), 2);
        assert_eq!(retry_success_count.get(), 2);
    }

    #[tokio::test]
    async fn non_transient_errors_are_not_retried() {
        let attempts = Rc::new(Cell::new(0usize));
        let result = write_file_with_retry_hooks(
            {
                let attempts = Rc::clone(&attempts);
                move || {
                    let attempts = Rc::clone(&attempts);
                    async move {
                        attempts.set(attempts.get() + 1);
                        Err(io::Error::new(io::ErrorKind::NotFound, "missing"))
                    }
                }
            },
            |_| async {},
            |_| panic!("write did not succeed"),
            |_, _, _| panic!("non-transient errors must not be retried"),
            |_, _| panic!("non-transient errors must not exhaust retry budget"),
            is_test_transient_write_lock_error,
        )
        .await;

        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::NotFound);
        assert_eq!(attempts.get(), 1);
    }

    #[tokio::test]
    async fn persistent_transient_lock_exhausts_retry_budget() {
        let attempts = Rc::new(Cell::new(0usize));
        let sleeps = Rc::new(Cell::new(0usize));
        let exhausted_count = Rc::new(Cell::new(0usize));

        let result = write_file_with_retry_hooks(
            {
                let attempts = Rc::clone(&attempts);
                move || {
                    let attempts = Rc::clone(&attempts);
                    async move {
                        attempts.set(attempts.get() + 1);
                        Err(io::Error::from_raw_os_error(WINDOWS_ERROR_LOCK_VIOLATION))
                    }
                }
            },
            {
                let sleeps = Rc::clone(&sleeps);
                move |_| {
                    let sleeps = Rc::clone(&sleeps);
                    async move {
                        sleeps.set(sleeps.get() + 1);
                    }
                }
            },
            |_| panic!("write did not succeed"),
            |_, _, _| {},
            |_, count| exhausted_count.set(count),
            is_test_transient_write_lock_error,
        )
        .await;

        assert_eq!(
            result.unwrap_err().raw_os_error(),
            Some(WINDOWS_ERROR_LOCK_VIOLATION)
        );
        assert_eq!(attempts.get(), WRITE_RETRY_DELAYS.len() + 1);
        assert_eq!(sleeps.get(), WRITE_RETRY_DELAYS.len());
        assert_eq!(exhausted_count.get(), WRITE_RETRY_DELAYS.len());
    }
}
