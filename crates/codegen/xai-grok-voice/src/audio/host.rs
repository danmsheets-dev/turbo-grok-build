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
//! A `debug_assert!` on the host's thread id turns that mistake into a failing
//! test instead of a silent, permanent, process-wide audio deadlock.
//!
//! # Nothing here waits forever
//!
//! Funnelling every `cpal` call through one thread also funnels the blast
//! radius: a single wedged call (cpal's WASAPI `Stream::drop` joins the device
//! thread and can hang on a misbehaving driver) would otherwise wedge every
//! later audio call in the process. Every wait below is therefore bounded —
//! callers get a `VoiceError` (or, for a release, a leaked stream id and a
//! logged error) rather than blocking forever. Teardown paths reach this
//! synchronously from inside async tasks, so an unbounded wait here would park
//! an executor thread for good.

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
/// Blocks until the host has finished, or until the host has been unresponsive
/// long enough that waiting is worse than failing — then it returns
/// `VoiceError::Config`. A panic inside `f` (e.g. `cpal`'s
/// `CoCreateInstance().unwrap()`) is re-raised on the calling thread rather
/// than killing the host.
pub(crate) fn call<T, F>(f: F) -> Result<T, VoiceError>
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
    use std::sync::mpsc::{RecvTimeoutError, Sender, channel, sync_channel};
    use std::thread::ThreadId;
    use std::time::Duration;

    use crate::error::VoiceError;

    /// How long a caller waits for a device enumeration or a stream build.
    ///
    /// Generous: WASAPI enumeration on a machine with a stalled driver is slow
    /// but not unbounded, and the capture start-up handshake above this layer
    /// already gives up at 5 s. This is the backstop for "never returns".
    const CALL_TIMEOUT: Duration = Duration::from_secs(10);

    /// How long teardown waits for the device to actually be released.
    ///
    /// Shorter, because a human is waiting: `CaptureHandle::stop()` joins the
    /// capture thread, whose last act is dropping the stream through here, and
    /// `stop()` is awaited from async tasks.
    const RELEASE_TIMEOUT: Duration = Duration::from_secs(5);

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
        ///
        /// Bounded: if the host does not confirm within [`RELEASE_TIMEOUT`] the
        /// id is left queued (the stream leaks, and is dropped whenever the host
        /// unsticks) rather than parking the caller — which is a teardown path
        /// reached synchronously from async tasks.
        pub(super) fn release(&mut self) {
            host().release(self.id);
        }
    }

    /// The host thread plus everything needed to talk to it.
    ///
    /// One process-wide instance ([`host`]); tests build private ones so they
    /// can wedge a host without poisoning the real one.
    pub(super) struct Host {
        tx: Sender<Job>,
        /// Identity of the host thread, so a reentrant call can be caught.
        thread_id: ThreadId,
        call_timeout: Duration,
        release_timeout: Duration,
    }

    impl Host {
        /// Spawn a host thread that never exits.
        ///
        /// The caller keeps the `Sender`, so the host's `recv()` never
        /// disconnects — which is the entire point: its COM apartment must
        /// outlive every WASAPI object `cpal` caches in its process-global
        /// enumerator.
        pub(super) fn spawn(call_timeout: Duration, release_timeout: Duration) -> Self {
            let (tx, rx) = channel::<Job>();
            let handle = std::thread::Builder::new()
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
            let thread_id = handle.thread().id();
            Self {
                tx,
                thread_id,
                call_timeout,
                release_timeout,
            }
        }

        /// Reentrancy guard for the rule documented at the top of this module.
        ///
        /// Calling back into the host from a closure already running on it waits
        /// for a thread that is waiting for you: a permanent, process-wide,
        /// silent audio deadlock (or, now, a `CALL_TIMEOUT` stall on every audio
        /// call for the rest of the session). Assert instead, so it shows up as
        /// a failing test rather than a hang in the field.
        fn assert_not_reentrant(&self, what: &str) {
            debug_assert!(
                std::thread::current().id() != self.thread_id,
                "reentrant audio-host call ({what}): the audio host cannot service itself"
            );
        }

        pub(super) fn call<T, F>(&self, f: F) -> Result<T, VoiceError>
        where
            F: FnOnce() -> T + Send + 'static,
            T: Send + 'static,
        {
            self.assert_not_reentrant("call");
            let (reply_tx, reply_rx) = sync_channel::<T>(1);
            let job: Job = Box::new(move |_| {
                let _ = reply_tx.send(f());
            });
            self.tx.send(job).expect("audio host thread accepts jobs");
            match reply_rx.recv_timeout(self.call_timeout) {
                Ok(value) => Ok(value),
                Err(RecvTimeoutError::Disconnected) => panic!("audio host job panicked"),
                Err(RecvTimeoutError::Timeout) => Err(self.wedged("call")),
            }
        }

        pub(super) fn open_stream<T, F>(&self, build: F) -> Result<(Hosted, T), VoiceError>
        where
            F: FnOnce() -> Result<(cpal::Stream, T), VoiceError> + Send + 'static,
            T: Send + 'static,
        {
            static NEXT_ID: AtomicU64 = AtomicU64::new(0);
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);

            self.assert_not_reentrant("open_stream");
            let (reply_tx, reply_rx) = sync_channel::<Result<T, VoiceError>>(1);
            let job: Job = Box::new(move |streams| {
                let reply = build().map(|(stream, extra)| {
                    streams.insert(id, stream);
                    extra
                });
                let _ = reply_tx.send(reply);
            });
            self.tx.send(job).expect("audio host thread accepts jobs");

            let extra = match reply_rx.recv_timeout(self.call_timeout) {
                Ok(reply) => reply?,
                Err(RecvTimeoutError::Disconnected) => panic!("audio host job panicked"),
                // The build may still land later and park a live stream under
                // `id` that nobody will ever release. Leaking one stream on a
                // host that has already stopped responding beats never
                // returning to the caller.
                Err(RecvTimeoutError::Timeout) => return Err(self.wedged("open_stream")),
            };
            Ok((Hosted { id }, extra))
        }

        pub(super) fn release(&self, id: u64) {
            self.assert_not_reentrant("release");
            let (done_tx, done_rx) = sync_channel::<()>(1);
            let job: Job = Box::new(move |streams| {
                drop(streams.remove(&id));
                let _ = done_tx.send(());
            });
            if self.tx.send(job).is_err() {
                return;
            }
            if done_rx.recv_timeout(self.release_timeout) == Err(RecvTimeoutError::Timeout) {
                tracing::error!(
                    stream_id = id,
                    timeout_ms = self.release_timeout.as_millis() as u64,
                    "audio host did not release the stream in time; leaking it and continuing \
                     (the device may stay busy until the host unsticks)"
                );
            }
        }

        /// Park the host thread until the returned sender fires (or 30 s pass),
        /// simulating a `cpal` call that never comes back.
        ///
        /// Test-only, and deliberately not routed through [`Self::call`]: the
        /// point is to occupy the host *without* the caller waiting for it.
        #[cfg(test)]
        pub(super) fn wedge_for_test(&self) -> std::sync::mpsc::Sender<()> {
            let (unwedge_tx, unwedge_rx) = channel::<()>();
            let job: Job = Box::new(move |_| {
                let _ = unwedge_rx.recv_timeout(Duration::from_secs(30));
            });
            self.tx.send(job).expect("audio host thread accepts jobs");
            unwedge_tx
        }

        /// Log and describe an expired wait. Shared so every timeout reads the
        /// same in logs and in the toast the caller ends up showing.
        fn wedged(&self, what: &str) -> VoiceError {
            let ms = self.call_timeout.as_millis() as u64;
            tracing::error!(
                operation = what,
                timeout_ms = ms,
                "audio host did not respond; a cpal call is wedged and audio is unavailable \
                 for the rest of this session"
            );
            VoiceError::Config(format!(
                "audio host did not respond within {ms} ms ({what}); \
                 the audio device driver appears to be stuck"
            ))
        }
    }

    /// The process-wide host, created on first use.
    fn host() -> &'static Host {
        static HOST: OnceLock<Host> = OnceLock::new();
        HOST.get_or_init(|| Host::spawn(CALL_TIMEOUT, RELEASE_TIMEOUT))
    }

    pub(super) fn call<T, F>(f: F) -> Result<T, VoiceError>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        host().call(f)
    }

    pub(super) fn open_stream<T, F>(build: F) -> Result<(Hosted, T), VoiceError>
    where
        F: FnOnce() -> Result<(cpal::Stream, T), VoiceError> + Send + 'static,
        T: Send + 'static,
    {
        host().open_stream(build)
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

    /// Inline, so there is no host to be unresponsive: the `Result` exists only
    /// to keep one signature across platforms.
    pub(super) fn call<T, F>(f: F) -> Result<T, VoiceError>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        Ok(f())
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
        assert_eq!(call(|| 6 * 7).expect("healthy host answers"), 42);
    }

    /// The host is process-global, so it has to keep serving after any one
    /// caller's thread is gone — that is the whole point of the WASAPI fix.
    #[test]
    fn call_serves_a_run_of_short_lived_threads() {
        for i in 0..8u32 {
            let got = std::thread::spawn(move || call(move || i * 2))
                .join()
                .expect("caller thread should not panic")
                .expect("healthy host answers");
            assert_eq!(got, i * 2);
        }
    }

    /// Every audio call in the process now funnels through one thread, so a
    /// single wedged `cpal` call (WASAPI's `Stream::drop` joins the device
    /// thread and can hang on a bad driver) would wedge *every* later audio
    /// call — including `CaptureHandle::stop()`, which async tasks await.
    /// A later caller must fail, not hang.
    ///
    /// Runs against a private host: wedging the process-global one would break
    /// every other test in this binary, which is exactly the blast radius the
    /// timeout exists to bound.
    #[cfg(target_os = "windows")]
    #[test]
    fn a_wedged_job_cannot_block_a_later_caller_forever() {
        use std::time::{Duration, Instant};

        let budget = Duration::from_millis(250);
        let host = imp::Host::spawn(budget, budget);
        let unwedge = host.wedge_for_test();

        let started = Instant::now();
        let outcome = host.call(|| 6 * 7);
        let waited = started.elapsed();

        assert!(
            outcome.is_err(),
            "a caller queued behind a wedged job must fail, not receive a value"
        );
        assert!(
            waited < Duration::from_secs(5),
            "caller waited {waited:?} on a wedged host; the wait is not bounded"
        );

        // Let the host thread finish so it is not left parked for 30 s.
        let _ = unwedge.send(());
    }

    /// The reentrancy rule at the top of this module is otherwise unenforced,
    /// and breaking it is a silent permanent process-wide audio deadlock.
    /// `debug_assert!` makes it a panic on the host thread, which the caller
    /// re-raises — a failing test instead of a hang.
    ///
    /// Debug-only: with `debug_assert!` compiled out the inner call really does
    /// wait for the host to service itself, and only the timeout saves it.
    #[cfg(all(target_os = "windows", debug_assertions))]
    #[test]
    fn a_reentrant_call_panics_instead_of_deadlocking() {
        let outcome = std::panic::catch_unwind(|| call(|| call(|| 6 * 7)));
        assert!(
            outcome.is_err(),
            "a call issued from inside a host closure must be rejected loudly"
        );
        // The guard must not have taken the process-global host down with it.
        assert_eq!(call(|| "still here").expect("host survives"), "still here");
    }

    /// A panicking job (e.g. cpal's `CoCreateInstance().unwrap()`) must surface
    /// on the caller's thread without taking the host down with it — otherwise
    /// one bad call would wedge every later audio call in the process.
    #[test]
    fn a_panicking_job_does_not_kill_the_host() {
        let panicked = std::panic::catch_unwind(|| call(|| panic!("boom"))).is_err();
        assert!(panicked, "the panic should reach the caller");
        assert_eq!(call(|| "still here").expect("host survives"), "still here");
    }
}
