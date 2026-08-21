//! Microphone capture (optional `audio` feature).
//!
//! Three backends share one interface (`spawn_pcm_capture`,
//! `capture_pcm_for_duration`, `input_device_info`, `CaptureHandle`):
//!
//! - **Linux**: a subprocess recorder (`pw-record`/`parec`/`arecord`) — the
//!   static-musl release binary cannot link `cpal` → `alsa-sys`; see
//!   [`capture_linux`].
//! - **macOS**: a subprocess too — the self-exec `__mic-capture` helper —
//!   because in-process CoreAudio memory is never returned after the stream
//!   drops; see [`capture_subprocess`].
//! - **Windows**: `cpal` (WASAPI) in-process; its memory cost is modest.
//!
//! Every in-process `cpal` call — capture here and live speaker playback in
//! `crate::live` — is funnelled through [`host`], which on Windows owns them
//! all from one thread that never exits; see that module for the WASAPI
//! use-after-free that requires.
//!
//! The fixed-duration probe capture stays in-process on macOS/Windows: it only
//! runs in short-lived diagnostic processes, where the memory dies at exit.
//!
//! `CaptureHandle` is deliberately one name per platform, resolved by the
//! re-exports below:
//! - Linux → `pipe::ChildCaptureHandle` (recorder subprocess);
//! - macOS → `capture_subprocess::CaptureHandle`, an enum over the helper
//!   subprocess and the in-process fallback;
//! - Windows → `capture::CaptureHandle` (in-process cpal stream).

// Single-owner host for every in-process `cpal` call. On Windows it is a
// dedicated thread that never exits, which is what keeps cpal's process-global
// WASAPI enumerator inside a live COM apartment; elsewhere it runs inline. See
// [`host`] for the use-after-free it prevents.
#[cfg(not(target_os = "linux"))]
pub(crate) mod host;
// cpal-based capture: the Windows backend, the macOS fallback, and the macOS
// `__mic-capture` child implementation.
#[cfg(not(target_os = "linux"))]
mod pcm;
#[cfg(not(target_os = "linux"))]
mod capture;
#[cfg(target_os = "windows")]
mod loopback;
// Wire protocol shared by the `__mic-capture` child (writer, in `capture`)
// and the macOS parent (parser, in `capture_subprocess`).
#[cfg(not(target_os = "linux"))]
mod protocol;
#[cfg(not(target_os = "linux"))]
pub use capture::capture_pcm_for_duration;
#[cfg(not(target_os = "linux"))]
pub(crate) use capture::run_capture_child_cli;
#[cfg(target_os = "windows")]
pub use capture::{CaptureHandle, input_device_info, spawn_pcm_capture};

/// What a meeting capture session actually opened.
#[derive(Debug, Clone)]
pub struct MeetingCaptureReport {
    pub used_loopback: bool,
    pub mic: bool,
    pub device_labels: Vec<String>,
}

/// Capture for `/meeting`: Windows prefers WASAPI loopback (all participants)
/// mixed with the mic; other OS / failures fall back to the default microphone.
pub fn spawn_meeting_pcm_capture(
    sample_rate: u32,
    pcm_tx: tokio::sync::mpsc::Sender<Vec<u8>>,
    prefer_loopback: bool,
    include_mic: bool,
) -> Result<(CaptureHandle, MeetingCaptureReport), crate::error::VoiceError> {
    #[cfg(target_os = "windows")]
    {
        if prefer_loopback {
            match loopback::spawn_loopback_mix(sample_rate, pcm_tx.clone(), include_mic) {
                Ok((handle, report)) => {
                    return Ok((
                        handle,
                        MeetingCaptureReport {
                            used_loopback: report.used_loopback,
                            mic: report.mic,
                            device_labels: report.device_labels,
                        },
                    ));
                }
                Err(e) => {
                    if !include_mic {
                        return Err(e);
                    }
                    tracing::warn!(
                        error = %e,
                        "WASAPI loopback failed; falling back to microphone"
                    );
                }
            }
        }
    }
    let _ = (prefer_loopback, include_mic);
    let handle = spawn_pcm_capture(sample_rate, pcm_tx)?;
    Ok((
        handle,
        MeetingCaptureReport {
            used_loopback: false,
            mic: true,
            device_labels: vec!["microphone".into()],
        },
    ))
}

// Shared PCM-over-pipe plumbing for the two subprocess backends.
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod pipe;

#[cfg(target_os = "macos")]
mod capture_subprocess;
#[cfg(target_os = "macos")]
pub use capture_subprocess::{CaptureHandle, input_device_info, spawn_pcm_capture};

#[cfg(target_os = "linux")]
mod capture_linux;
#[cfg(target_os = "linux")]
pub use capture_linux::{
    CaptureHandle, capture_pcm_for_duration, input_device_info, spawn_pcm_capture,
};
