use thiserror::Error;

#[derive(Debug, Error)]
pub enum VoiceError {
    #[error("configuration: {0}")]
    Config(String),

    #[error("STT: {0}")]
    Stt(String),

    #[error("auth: {0}")]
    Auth(String),

    #[error("WebSocket: {0}")]
    WebSocket(String),

    /// Local TTS (Windows SAPI) is not available on this platform/build.
    #[error("TTS unavailable: {0}")]
    TtsUnavailable(String),

    /// Local TTS ran and failed (SAPI COM / Speak error).
    #[error("TTS: {0}")]
    Tts(String),
}
