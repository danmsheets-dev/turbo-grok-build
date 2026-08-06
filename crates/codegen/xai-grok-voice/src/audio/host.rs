//! Single-owner host for every in-process `cpal` interaction.
//!
//! # Why this exists
//!
//! `cpal` 0.15 caches the WASAPI `IMMDeviceEnumerator` in a process-global
//! `OnceLock` but calls `com_initialized()` only *inside* `get_or_init`
//! (`host/wasapi/device.rs`). The enumerator is therefore created in the COM
//! apartment of whichever thread touched `cpal` first — and `cpal`'s
//! `ComInitialized` guard is a `thread_local` whose `Drop` runs
//! `CoUninitialize()`. When that thread exits, the apartment is torn down and
//! `MMDevAPI.dll` unmapped while the static keeps the now-dangling interface
//! pointer. The next `cpal` call from any thread dereferences freed memory:
//! `EXCEPTION_ACCESS_VIOLATION` (`0xc0000005`, reported as exit 139 by a POSIX
//! shell) — no panic, no unwind, nothing to catch.
//!
//! Our capture and playback backends each used to spawn a short-lived thread
//! that called into `cpal` and then exited, so the *second* dictation in a
//! session crashed the process. Upstream `cpal` still ships the same code on
//! master, so the fix has to live here.
//!
//! # The fix
//!
//! On Windows all `cpal` work runs on one dedicated host thread that is
//! spawned lazily and **never exits**. Its COM apartment therefore outlives
//! every WASAPI object `cpal` caches, and — because it is the only thread that
//! ever calls `cpal` — every call also happens in the apartment that created
//! those objects. That removes the whole bug class rather than one instance,
//! and it needs no fork of `cpal`.
//!
//! `cpal::Stream` is `!Send` (WASAPI's is) and must be dropped where it was
//! built, so streams stay on the host thread too: [`open_stream`] builds
//! one there and hands back a [`HostedStream`] token whose `Drop` asks the host
//! to release it, blocking until it has (callers such as
//! `CaptureHandle::stop()` promise the device is free when they return).
//!
//! # Other platforms
//!
//! Linux links no `cpal` at all (it shells out to a system recorder), so this
//! module is not compiled there. macOS opens `cpal` in a short-lived
//! `__mic-capture` child and has no equivalent apartment to lose, so both entry
//! points run the closure inline on the calling thread — the same thread, at
//! the same point in the sequence, as before this module existed.
//!
//! Callers must not invoke [`call`] or [`open_stream`] (or drop a
//! [`HostedStream`]) from inside a closure already running on the host: on
//! Windows that would deadlock waiting for the host thread to service itself.

use crate::error::VoiceError;

/// A `cpal` stream owned by the audio host.
///
/// Dropping it stops the stream and releases the device before returning.
pub(crate) struct HostedStream(imp::Hosted);

impl Drop for HostedStream {
    fn drop(&mut self) {
        self.0.release();
    }
}

/// Run `f` on the audio host and return its result.
///
/// Blocks until the host has finished. A panic inside `f` (e.g. `cpal`'s
/// `CoCreateInstance().unwrap()`) is re-raised on the calling thread rather
/// than killing the host.
pub(crate) fn call<T, F>(f: F) -> T
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    imp::call(f)
}

/// Build and start a `cpal` stream on the audio host, where it stays until the
/// returned [`HostedStream`] is dropped.
///
/// `build` must return the started stream plus whatever extra value the caller
/// needs back (a device name, `()`, …); it runs entirely on the host, so the
/// stream itself never crosses a thread boundary.
pub(crate) fn open_stream<T, F>(build: F) -> Result<(HostedStream, T), VoiceError>
where
    F: FnOnce() -> Result<(cpal::Stream, T), VoiceError> + Send + 'static,
    T: Send + 'static,
{
    let (hosted, extra) = imp::open_stream(build)?;
    Ok((HostedStream(hosted), extra))
}

// ---------------------------------------------------------------------------
// Windows: one immortal host thread owns every `cpal` object.
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
mod imp {
    use std::collections::HashMap;
    use std::panic::AssertUnwindSafe;
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::mpsc::{Sender, channel, sync_channel};

    use crate::error::VoiceError;

    /// Streams currently alive on the host, keyed by [`Hosted::id`].
    type Streams = HashMap<u64, cpal::Stream>;
    /// One unit of work for the host thread.
    type Job = Box<dyn FnOnce(&mut Streams) + Send>;

    /// Handle to a stream parked on the host thread.
    pub(super) struct Hosted {
        id: u64,
    }

    impl Hosted {
        /// Drop the stream on the host and wait for it, so the device really is
        /// released by the time the caller continues.
        pub(super) fn release(&mut self) {
            let id = self.id;
            let (done_tx, done_rx) = sync_channel::<()>(1);
            let job: Job = Box::new(move |streams| {
                drop(streams.remove(&id));
                let _ = done_tx.send(());
            });
            if sender().send(job).is_ok() {
                let _ = done_rx.recv();
            }
        }
    }

    /// Channel to the host thread, created on first use.
    ///
    /// The `OnceLock` keeps the sender alive for the whole process, so the
    /// host's `recv()` never disconnects and the thread never exits — which is
    /// the entire point: its COM apartment must outlive every WASAPI object
    /// `cpal` caches in its process-global enumerator.
    fn sender() -> &'static Sender<Job> {
        static HOST: OnceLock<Sender<Job>> = OnceLock::new();
        HOST.get_or_init(|| {
            let (tx, rx) = channel::<Job>();
            std::thread::Builder::new()
                .name("grok-audio-host".to_string())
                .spawn(move || {
                    let mut streams: Streams = Streams::new();
                    while let Ok(job) = rx.recv() {
                        // Contain panics: if the host died, every later audio
                        // call would block forever. The caller observes the
                        // dropped reply channel and re-panics on its own
                        // thread, which is where the panic used to surface.
                        let _ = std::panic::catch_unwind(AssertUnwindSafe(|| job(&mut streams)));
                    }
                })
                .expect("spawn audio host thread");
            tx
        })
    }

    pub(super) fn call<T, F>(f: F) -> T
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        let (reply_tx, reply_rx) = sync_channel::<T>(1);
        let job: Job = Box::new(move |_| {
            let _ = reply_tx.send(f());
        });
        sender().send(job).expect("audio host thread accepts jobs");
        match reply_rx.recv() {
            Ok(value) => value,
            Err(_) => panic!("audio host job panicked"),
        }
    }

    pub(super) fn open_stream<T, F>(build: F) -> Result<(Hosted, T), VoiceError>
    where
        F: FnOnce() -> Result<(cpal::Stream, T), VoiceError> + Send + 'static,
        T: Send + 'static,
    {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);

        let (reply_tx, reply_rx) = sync_channel::<Result<T, VoiceError>>(1);
        let job: Job = Box::new(move |streams| {
            let reply = build().map(|(stream, extra)| {
                streams.insert(id, stream);
                extra
            });
            let _ = reply_tx.send(reply);
        });
        sender().send(job).expect("audio host thread accepts jobs");

        let extra = match reply_rx.recv() {
            Ok(reply) => reply?,
            Err(_) => panic!("audio host job panicked"),
        };
        Ok((Hosted { id }, extra))
    }
}

// ---------------------------------------------------------------------------
// Everything else: run inline, exactly where the caller used to.
// ---------------------------------------------------------------------------

#[cfg(not(target_os = "windows"))]
mod imp {
    use crate::error::VoiceError;

    /// The stream itself — nothing is marshalled anywhere on these platforms.
    pub(super) struct Hosted(Option<cpal::Stream>);

    impl Hosted {
        pub(super) fn release(&mut self) {
            drop(self.0.take());
        }
    }

    pub(super) fn call<T, F>(f: F) -> T
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        f()
    }

    pub(super) fn open_stream<T, F>(build: F) -> Result<(Hosted, T), VoiceError>
    where
        F: FnOnce() -> Result<(cpal::Stream, T), VoiceError> + Send + 'static,
        T: Send + 'static,
    {
        let (stream, extra) = build()?;
        Ok((Hosted(Some(stream)), extra))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn call_returns_the_closure_result() {
        assert_eq!(call(|| 6 * 7), 42);
    }

    /// The host is process-global, so it has to keep serving after any one
    /// caller's thread is gone — that is the whole point of the WASAPI fix.
    #[test]
    fn call_serves_a_run_of_short_lived_threads() {
        for i in 0..8u32 {
            let got = std::thread::spawn(move || call(move || i * 2))
                .join()
                .expect("caller thread should not panic");
            assert_eq!(got, i * 2);
        }
    }

    /// A panicking job (e.g. cpal's `CoCreateInstance().unwrap()`) must surface
    /// on the caller's thread without taking the host down with it — otherwise
    /// one bad call would wedge every later audio call in the process.
    #[test]
    fn a_panicking_job_does_not_kill_the_host() {
        let panicked = std::panic::catch_unwind(|| call(|| panic!("boom"))).is_err();
        assert!(panicked, "the panic should reach the caller");
        assert_eq!(call(|| "still here"), "still here");
    }
}
