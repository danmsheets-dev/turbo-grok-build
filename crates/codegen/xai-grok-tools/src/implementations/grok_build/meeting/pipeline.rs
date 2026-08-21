//! Capture (loopback+mic or mic) → Grok streaming STT → transcript.jsonl.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::Utc;
use tokio::sync::mpsc;
use xai_grok_meetings::{CaptureSource, MeetingStore, TranscriptSegment, extract_turbo_question};
use xai_grok_voice::auth::SharedVoiceAuth;
use xai_grok_voice::config::VoiceConfig;
use xai_grok_voice::stt::{StreamingSttEvent, StreamingSttSession};

use crate::notification::types::ToolNotificationHandle;

use super::auto_ask;

/// Env: skip WASAPI/mic (unit tests, CI).
pub const NO_CAPTURE_ENV: &str = "GROK_MEETING_NO_CAPTURE";
/// `mic` = microphone only; `loopback` = system mix without mic; unset = auto.
pub const CAPTURE_PREF_ENV: &str = "GROK_MEETING_CAPTURE";

pub fn no_capture_requested() -> bool {
    matches!(
        std::env::var(NO_CAPTURE_ENV).as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE")
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapturePref {
    Auto,
    Microphone,
    LoopbackOnly,
}

pub fn capture_pref_from_env() -> CapturePref {
    match std::env::var(CAPTURE_PREF_ENV) {
        Ok(s) => {
            let l = s.trim().to_ascii_lowercase();
            if matches!(l.as_str(), "mic" | "microphone") {
                CapturePref::Microphone
            } else if matches!(l.as_str(), "loopback" | "speakers" | "mix") {
                CapturePref::LoopbackOnly
            } else {
                CapturePref::Auto
            }
        }
        Err(_) => CapturePref::Auto,
    }
}

/// Pick capture path for this process.
pub fn choose_capture_source() -> CaptureSource {
    choose_capture_source_with(no_capture_requested(), cfg!(test), capture_pref_from_env())
}

pub fn choose_capture_source_with(
    no_capture: bool,
    is_test: bool,
    pref: CapturePref,
) -> CaptureSource {
    if no_capture || is_test {
        return CaptureSource::None;
    }
    match pref {
        CapturePref::Microphone => CaptureSource::Microphone,
        CapturePref::LoopbackOnly | CapturePref::Auto if cfg!(target_os = "windows") => {
            CaptureSource::Loopback
        }
        _ => CaptureSource::Microphone,
    }
}

#[cfg(test)]
mod capture_pref_tests {
    use super::*;

    #[test]
    fn tests_force_none() {
        assert_eq!(
            choose_capture_source_with(false, true, CapturePref::Auto),
            CaptureSource::None
        );
        assert_eq!(
            choose_capture_source_with(true, false, CapturePref::Auto),
            CaptureSource::None
        );
    }

    #[test]
    fn mic_pref_is_microphone() {
        assert_eq!(
            choose_capture_source_with(false, false, CapturePref::Microphone),
            CaptureSource::Microphone
        );
    }

    #[test]
    fn loopback_only_does_not_become_none() {
        let src = choose_capture_source_with(false, false, CapturePref::LoopbackOnly);
        if cfg!(target_os = "windows") {
            assert_eq!(src, CaptureSource::Loopback);
        } else {
            assert_eq!(src, CaptureSource::Microphone);
        }
    }
}

/// Run until `stop` is set. Reconnects STT if the socket ends.
pub async fn run_stt_loop(
    store: MeetingStore,
    auth: SharedVoiceAuth,
    config: VoiceConfig,
    mut pcm_rx: mpsc::Receiver<Vec<u8>>,
    stop: Arc<AtomicBool>,
    notification: Option<ToolNotificationHandle>,
) {
    let mut spoken: HashSet<String> = HashSet::new();
    while !stop.load(Ordering::Relaxed) {
        let bearer = match auth.bearer().await {
            Some(b) if !b.is_empty() => b,
            _ => {
                tracing::warn!("meeting STT auth: not signed in");
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                continue;
            }
        };
        let mut stt = match StreamingSttSession::connect(&config, &bearer).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("meeting STT connect: {e}");
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                continue;
            }
        };

        loop {
            if stop.load(Ordering::Relaxed) {
                stt.finish_audio();
                return;
            }
            tokio::select! {
                chunk = pcm_rx.recv() => {
                    let Some(bytes) = chunk else { return };
                    if stt.send_pcm(bytes).await.is_err() {
                        break;
                    }
                }
                ev = stt.recv() => {
                    match ev {
                        Some(StreamingSttEvent::Partial(p)) if !p.text.trim().is_empty() => {
                            let is_final = p.is_final || p.speech_final;
                            if is_final {
                                maybe_queue_spoken(
                                    &store,
                                    notification.as_ref(),
                                    &mut spoken,
                                    &p.text,
                                );
                            }
                            let _ = store.append_segment(&TranscriptSegment {
                                at: Utc::now(),
                                text: p.text,
                                is_final,
                            });
                        }
                        Some(StreamingSttEvent::Done { text }) => {
                            if !text.trim().is_empty() {
                                maybe_queue_spoken(
                                    &store,
                                    notification.as_ref(),
                                    &mut spoken,
                                    &text,
                                );
                                let _ = store.append_segment(&TranscriptSegment {
                                    at: Utc::now(),
                                    text,
                                    is_final: true,
                                });
                            }
                            break;
                        }
                        Some(StreamingSttEvent::Error { message }) => {
                            tracing::warn!("meeting STT: {message}");
                            break;
                        }
                        Some(StreamingSttEvent::Ready) => {}
                        None => break,
                        _ => {}
                    }
                }
            }
        }
    }
}

fn maybe_queue_spoken(
    store: &MeetingStore,
    notification: Option<&ToolNotificationHandle>,
    seen: &mut HashSet<String>,
    text: &str,
) {
    let Some(q) = extract_turbo_question(text) else {
        return;
    };
    let key = q.trim().to_ascii_lowercase();
    if key.is_empty() || !seen.insert(key) {
        return;
    }
    let _ = store.enqueue_question("transcript", &q);
    if auto_ask::emit_auto_ask(notification, "transcript", &q) {
        let _ = store.mark_question_answered("transcript", &q);
    }
}

#[cfg(test)]
mod spoken_dedup_tests {
    use super::*;
    use xai_grok_meetings::{CaptureSource, MeetingStore, parse_meeting_url};

    #[test]
    fn duplicate_final_and_done_queue_once() {
        let root = std::env::temp_dir().join(format!(
            "turbo-stt-dedup-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let url = parse_meeting_url("https://teams.microsoft.com/l/meetup-join/x").unwrap();
        let store = MeetingStore::create(&root, "teams-dedup", &url, CaptureSource::None).unwrap();
        let mut seen = HashSet::new();
        maybe_queue_spoken(&store, None, &mut seen, "Turbo: How is the website?");
        maybe_queue_spoken(&store, None, &mut seen, "TURBO: How is the website?");
        assert_eq!(store.pending_question_count(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }
}
