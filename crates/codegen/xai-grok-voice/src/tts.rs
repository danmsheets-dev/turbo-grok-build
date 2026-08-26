//! Local text-to-speech for meeting notetaker answers.
//!
//! There is **no** xAI TTS client in this crate. Cloud endpoints are not
//! invented. When [`MEETING_TTS_ENV`] (`GROK_MEETING_TTS`) is set to `1`,
//! [`maybe_speak_reply`] speaks on **this PC's speakers** via Windows SAPI
//! (`ISpVoice`). Other platforms return [`TtsOutcome::Unavailable`].
//!
//! Playback is local one-shot speech, not injection into the Teams notetaker's
//! silent outbound WebRTC track.

use std::sync::{Arc, Mutex, OnceLock, RwLock};

use crate::error::VoiceError;

/// Opt-in env for speaking `meeting_reply` answers aloud.
pub const MEETING_TTS_ENV: &str = "GROK_MEETING_TTS";

/// Reason returned on non-Windows (and documented in status / reply output).
pub const TTS_UNAVAILABLE_REASON: &str =
    "local SAPI is Windows-only; no xAI TTS client exists in this crate";

/// Local synthesis backend. The only implemented backend is Windows SAPI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TtsBackend {
    WindowsSapi,
}

impl TtsBackend {
    pub fn label(self) -> &'static str {
        match self {
            Self::WindowsSapi => "Windows SAPI",
        }
    }
}

/// Result of [`maybe_speak_reply`] (the function `meeting_reply` calls).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TtsOutcome {
    Disabled,
    SkippedEmpty,
    Spoke { backend: TtsBackend, chars: usize },
    Unavailable { reason: String },
    Failed { reason: String },
}

/// Engine used by [`maybe_speak_reply`]. Tests install a mock.
pub trait TtsEngine: Send + Sync {
    fn speak(&self, text: &str) -> Result<TtsBackend, VoiceError>;
}

struct DefaultEngine;

impl TtsEngine for DefaultEngine {
    fn speak(&self, text: &str) -> Result<TtsBackend, VoiceError> {
        speak(text)
    }
}

static TEST_LOCK: Mutex<()> = Mutex::new(());
static ENGINE_OVERRIDE: OnceLock<RwLock<Option<Arc<dyn TtsEngine>>>> = OnceLock::new();

fn override_slot() -> &'static RwLock<Option<Arc<dyn TtsEngine>>> {
    ENGINE_OVERRIDE.get_or_init(|| RwLock::new(None))
}

fn current_engine() -> Arc<dyn TtsEngine> {
    override_slot()
        .read()
        .ok()
        .and_then(|g| g.clone())
        .unwrap_or_else(|| Arc::new(DefaultEngine))
}

/// Holds the TTS test lock and restores env + engine on drop.
pub struct TtsTestGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    prev_env: Option<String>,
}

impl TtsTestGuard {
    /// Serialize tests that mutate `GROK_MEETING_TTS` or the engine override.
    pub fn lock() -> Self {
        let lock = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev_env = std::env::var(MEETING_TTS_ENV).ok();
        Self {
            _lock: lock,
            prev_env,
        }
    }

    pub fn set_env(self, value: Option<&str>) -> Self {
        match value {
            Some(v) => unsafe { std::env::set_var(MEETING_TTS_ENV, v) },
            None => unsafe { std::env::remove_var(MEETING_TTS_ENV) },
        }
        self
    }

    pub fn set_engine(self, engine: Arc<dyn TtsEngine>) -> Self {
        if let Ok(mut slot) = override_slot().write() {
            *slot = Some(engine);
        }
        self
    }
}

impl Drop for TtsTestGuard {
    fn drop(&mut self) {
        match self.prev_env.take() {
            Some(prev) => unsafe { std::env::set_var(MEETING_TTS_ENV, prev) },
            None => unsafe { std::env::remove_var(MEETING_TTS_ENV) },
        }
        if let Ok(mut slot) = override_slot().write() {
            *slot = None;
        }
    }
}

/// Records `speak` calls. Used by voice tests and the `meeting_reply` mock path.
#[derive(Clone, Default)]
pub struct RecordingTtsEngine {
    spoken: Arc<Mutex<Vec<String>>>,
}

impl RecordingTtsEngine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn spoken(&self) -> Vec<String> {
        self.spoken
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

impl TtsEngine for RecordingTtsEngine {
    fn speak(&self, text: &str) -> Result<TtsBackend, VoiceError> {
        self.spoken
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(text.to_string());
        Ok(TtsBackend::WindowsSapi)
    }
}

/// `1` / `true` / `on` / `yes` (any case). Unset and any other value are off.
pub fn meeting_tts_enabled() -> bool {
    match std::env::var(MEETING_TTS_ENV) {
        Ok(s) => matches!(
            s.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "on" | "yes"
        ),
        Err(_) => false,
    }
}

/// Strip a leading `[Turbo]` so SAPI does not read the chat prefix aloud.
pub fn spoken_text(text: &str) -> &str {
    let t = text.trim();
    if t.len() >= 7 && t[..7].eq_ignore_ascii_case("[turbo]") {
        t[7..].trim()
    } else {
        t
    }
}

/// Speak `text` with the current engine (Windows SAPI unless a test mock).
pub fn speak(text: &str) -> Result<TtsBackend, VoiceError> {
    speak_platform(text)
}

/// The function `meeting_reply` calls. No-op unless `GROK_MEETING_TTS=1`.
pub fn maybe_speak_reply(text: &str) -> TtsOutcome {
    if !meeting_tts_enabled() {
        return TtsOutcome::Disabled;
    }
    let spoken = spoken_text(text);
    if spoken.is_empty() {
        return TtsOutcome::SkippedEmpty;
    }
    match current_engine().speak(spoken) {
        Ok(backend) => TtsOutcome::Spoke {
            backend,
            chars: spoken.chars().count(),
        },
        Err(VoiceError::TtsUnavailable(reason)) => TtsOutcome::Unavailable { reason },
        Err(e) => TtsOutcome::Failed {
            reason: e.to_string(),
        },
    }
}

/// Line appended to `meeting_reply` output. `None` when TTS is off.
pub fn format_tts_line(outcome: &TtsOutcome) -> Option<String> {
    match outcome {
        TtsOutcome::Disabled => None,
        TtsOutcome::SkippedEmpty => Some("TTS skipped: empty answer.".into()),
        TtsOutcome::Spoke { backend, .. } => Some(format!(
            "Spoke locally via {} (this PC's speakers; not injected into the meeting bot).",
            backend.label()
        )),
        TtsOutcome::Unavailable { reason } => Some(format!("TTS unavailable: {reason}")),
        TtsOutcome::Failed { reason } => Some(format!("TTS failed: {reason}")),
    }
}

/// One `meeting_status` line. Always present so off vs local-SAPI cannot be inferred.
pub fn format_tts_status_line() -> String {
    if !meeting_tts_enabled() {
        return "tts: off".into();
    }
    if cfg!(windows) {
        "tts: Windows SAPI (GROK_MEETING_TTS=1; this PC's speakers, not meeting bot audio)".into()
    } else {
        "tts: requested (GROK_MEETING_TTS=1) but local SAPI is Windows-only".into()
    }
}

#[cfg(windows)]
fn speak_platform(text: &str) -> Result<TtsBackend, VoiceError> {
    let text = text.to_string();
    std::thread::Builder::new()
        .name("grok-meeting-tts".into())
        .spawn(move || speak_sapi_sta(&text))
        .map_err(|e| VoiceError::Tts(format!("spawn SAPI thread: {e}")))?
        .join()
        .map_err(|_| VoiceError::Tts("SAPI thread panicked".into()))?
}

#[cfg(not(windows))]
fn speak_platform(_text: &str) -> Result<TtsBackend, VoiceError> {
    Err(VoiceError::TtsUnavailable(TTS_UNAVAILABLE_REASON.into()))
}

/// CLSID_SpVoice (`{96749377-3391-11D2-9EE3-00C04F797396}`).
#[cfg(windows)]
const CLSID_SP_VOICE: windows::core::GUID =
    windows::core::GUID::from_u128(0x9674_9377_3391_11d2_9ee3_00c0_4f79_7396);

#[cfg(windows)]
fn speak_sapi_sta(text: &str) -> Result<TtsBackend, VoiceError> {
    use windows::Win32::Media::Speech::{ISpVoice, SPF_DEFAULT, SPF_IS_NOT_XML};
    use windows::Win32::System::Com::{
        CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
    };
    use windows::core::HSTRING;

    unsafe {
        let initialized = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
        let result = (|| {
            let voice: ISpVoice = CoCreateInstance(&CLSID_SP_VOICE, None, CLSCTX_ALL)
                .map_err(|e| VoiceError::Tts(format!("SAPI CoCreateInstance: {e}")))?;
            let wide = HSTRING::from(text);
            let flags = (SPF_DEFAULT.0 | SPF_IS_NOT_XML.0) as u32;
            voice
                .Speak(&wide, flags, None)
                .map_err(|e| VoiceError::Tts(format!("SAPI Speak: {e}")))?;
            Ok(TtsBackend::WindowsSapi)
        })();
        if initialized {
            CoUninitialize();
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_name_is_stable() {
        assert_eq!(MEETING_TTS_ENV, "GROK_MEETING_TTS");
    }

    #[test]
    fn default_off_and_explicit_on() {
        let _g = TtsTestGuard::lock().set_env(None);
        assert!(!meeting_tts_enabled());
        drop(_g);

        let cases: &[(&str, bool)] = &[
            ("1", true),
            ("true", true),
            ("ON", true),
            ("Yes", true),
            ("0", false),
            ("false", false),
            ("off", false),
            ("no", false),
            ("", false),
            ("sapi", false),
        ];
        for (val, want) in cases {
            let _g = TtsTestGuard::lock().set_env(Some(val));
            assert_eq!(meeting_tts_enabled(), *want, "GROK_MEETING_TTS={val:?}");
        }
    }

    #[test]
    fn spoken_text_strips_turbo_prefix() {
        assert_eq!(spoken_text("  [Turbo] ships Friday  "), "ships Friday");
        assert_eq!(spoken_text("[turbo] hi"), "hi");
        assert_eq!(spoken_text("[TURBO] hi"), "hi");
        assert_eq!(spoken_text("no prefix"), "no prefix");
        assert_eq!(spoken_text("   "), "");
        assert_eq!(spoken_text("[Turbo]"), "");
    }

    #[test]
    fn meeting_reply_path_does_not_speak_when_disabled() {
        let mock = RecordingTtsEngine::new();
        let _g = TtsTestGuard::lock()
            .set_env(None)
            .set_engine(Arc::new(mock.clone()));
        let outcome = maybe_speak_reply("[Turbo] ships Friday");
        assert_eq!(outcome, TtsOutcome::Disabled);
        assert!(mock.spoken().is_empty());
        assert!(format_tts_line(&outcome).is_none());
    }

    #[test]
    fn meeting_reply_path_calls_speak_when_enabled() {
        let mock = RecordingTtsEngine::new();
        let _g = TtsTestGuard::lock()
            .set_env(Some("1"))
            .set_engine(Arc::new(mock.clone()));
        let outcome = maybe_speak_reply("[Turbo] The website ships Friday.");
        assert_eq!(
            outcome,
            TtsOutcome::Spoke {
                backend: TtsBackend::WindowsSapi,
                chars: "The website ships Friday.".chars().count(),
            }
        );
        assert_eq!(mock.spoken(), vec!["The website ships Friday.".to_string()]);
        let line = format_tts_line(&outcome).expect("spoke line");
        assert!(line.contains("Windows SAPI"), "{line}");
        assert!(line.contains("this PC's speakers"), "{line}");
        assert!(line.contains("not injected"), "{line}");
    }

    #[test]
    fn meeting_reply_path_skips_empty_after_prefix() {
        let mock = RecordingTtsEngine::new();
        let _g = TtsTestGuard::lock()
            .set_env(Some("1"))
            .set_engine(Arc::new(mock.clone()));
        assert_eq!(maybe_speak_reply("[Turbo]   "), TtsOutcome::SkippedEmpty);
        assert!(mock.spoken().is_empty());
    }

    #[test]
    fn default_engine_is_honest_without_a_cloud_endpoint() {
        let prod = include_str!("tts.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("tests module");
        assert!(
            !prod.contains("api.x.ai"),
            "do not invent an xAI TTS host in the TTS module"
        );
        assert!(!prod.contains("/v1/audio"), "{prod}");
        assert!(prod.contains("ISpVoice") || !cfg!(windows));
        // Do not call real SAPI in unit tests (it would play speakers).
        #[cfg(not(windows))]
        {
            let _g = TtsTestGuard::lock();
            match speak("hello") {
                Err(VoiceError::TtsUnavailable(reason)) => {
                    assert_eq!(reason, TTS_UNAVAILABLE_REASON);
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }

    #[test]
    fn status_line_names_local_sapi_or_off() {
        let _off = TtsTestGuard::lock().set_env(None);
        assert_eq!(format_tts_status_line(), "tts: off");
        drop(_off);

        let _on = TtsTestGuard::lock().set_env(Some("1"));
        let line = format_tts_status_line();
        if cfg!(windows) {
            assert!(line.contains("Windows SAPI"), "{line}");
            assert!(line.contains("not meeting bot"), "{line}");
        } else {
            assert!(line.contains("Windows-only"), "{line}");
        }
    }
}
