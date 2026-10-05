//! Speech APIs of providers (§33 Voice Command Center): speech-to-text and
//! text-to-speech. Optional adapter capabilities — the [`ProviderAdapter`]
//! defaults return `Unsupported`; the OpenAI-wire adapter implements both.
//!
//! Callers (the core's `voice.*` handlers) enforce the user's consent before
//! any audio or text reaches a provider; this module only speaks the wire.
//!
//! Wire (pinned):
//! - `POST {base}/audio/transcriptions` multipart (`file`, `model`,
//!   `response_format=json`, optional `language`) → `{"text": "…"}`
//! - `POST {base}/audio/speech` JSON (`model`, `input`, `voice`,
//!   `response_format=mp3`, optional `instructions`) → audio bytes
//!
//! [`ProviderAdapter`]: crate::ProviderAdapter

use serde_json::{json, Value};

use crate::http::{self, HttpConfig};
use crate::types::ProviderError;

/// Default transcription model.
pub const DEFAULT_STT_MODEL: &str = "whisper-1";
/// Default speech model (supports `instructions` for style).
pub const DEFAULT_TTS_MODEL: &str = "gpt-4o-mini-tts";
/// Request timeout for speech calls.
pub const AUDIO_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// A transcription request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptionRequest {
    /// Encoded audio (WAV, WebM, MP3, …).
    pub audio: Vec<u8>,
    /// MIME type of `audio`.
    pub mime_type: String,
    /// Optional language hint (ISO-639-1).
    pub language: Option<String>,
    /// Model override.
    pub model: Option<String>,
}

/// A transcription result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transcription {
    /// Recognized text.
    pub text: String,
    /// Detected language, when reported.
    pub language: Option<String>,
}

/// A speech synthesis request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeechRequest {
    /// Text.
    pub text: String,
    /// Voice id (`alloy`, `verse`, …).
    pub voice: String,
    /// Style instructions.
    pub style: Option<String>,
    /// Model override.
    pub model: Option<String>,
}

/// Synthesized audio.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeechAudio {
    /// Encoded audio.
    pub bytes: Vec<u8>,
    /// MIME type.
    pub mime_type: String,
}

/// File extension for a MIME type (multipart file name).
#[must_use]
pub fn extension_for(mime: &str) -> &'static str {
    match mime.split(';').next().unwrap_or("").trim() {
        "audio/webm" => "webm",
        "audio/ogg" => "ogg",
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/mp4" | "audio/m4a" | "audio/x-m4a" => "m4a",
        "audio/flac" => "flac",
        _ => "wav",
    }
}

/// Builds a `multipart/form-data` body. Returns `(content_type, body)`.
#[must_use]
pub fn multipart_body(boundary: &str, fields: &[(&str, &str)], file: (&str, &str, &[u8])) -> (String, Vec<u8>) {
    let mut body = Vec::with_capacity(file.2.len() + 512);
    for (name, value) in fields {
        body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes());
        body.extend_from_slice(value.as_bytes());
        body.extend_from_slice(b"\r\n");
    }
    let (file_name, mime, bytes) = file;
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{file_name}\"\r\nContent-Type: {mime}\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

fn sanitize_mime(mime: &str) -> String {
    let m = mime.split(';').next().unwrap_or("").trim();
    if !m.is_empty() && m.len() <= 64 && m.chars().all(|c| c.is_ascii_alphanumeric() || "/-+.".contains(c)) {
        m.to_owned()
    } else {
        "audio/wav".to_owned()
    }
}

/// OpenAI-wire transcription.
pub async fn openai_transcribe(
    cfg: &HttpConfig,
    api_key: Option<String>,
    req: TranscriptionRequest,
) -> Result<Transcription, ProviderError> {
    if req.audio.is_empty() {
        return Err(ProviderError::BadRequest("empty audio".into()));
    }
    let mime = sanitize_mime(&req.mime_type);
    let model = req.model.unwrap_or_else(|| DEFAULT_STT_MODEL.to_owned());
    let mut fields: Vec<(&str, &str)> = vec![("model", model.as_str()), ("response_format", "json")];
    let lang = req.language.as_deref().map(|l| l.split('-').next().unwrap_or(l).to_ascii_lowercase());
    if let Some(l) = lang.as_deref().filter(|l| (2..=3).contains(&l.len()) && l.chars().all(|c| c.is_ascii_lowercase())) {
        fields.push(("language", l));
    }
    let boundary = format!("orkestr-{}", uuid_like());
    let file_name = format!("speech.{}", extension_for(&mime));
    let (content_type, body) = multipart_body(&boundary, &fields, (&file_name, &mime, &req.audio));
    let mut rb = cfg
        .client
        .post(cfg.url("audio/transcriptions"))
        .timeout(AUDIO_TIMEOUT)
        .header(reqwest::header::CONTENT_TYPE, content_type)
        .body(body);
    rb = cfg.with_headers(rb);
    if let Some(k) = api_key {
        rb = rb.bearer_auth(k);
    }
    let resp = http::send(rb).await?;
    let v: Value = resp.json().await.map_err(http::map_reqwest_error)?;
    let text = v
        .get("text")
        .and_then(Value::as_str)
        .ok_or_else(|| ProviderError::Server("transcription response has no `text`".into()))?;
    Ok(Transcription {
        text: text.trim().to_owned(),
        language: v.get("language").and_then(Value::as_str).map(str::to_owned),
    })
}

/// OpenAI-wire speech synthesis (MP3).
pub async fn openai_speech(
    cfg: &HttpConfig,
    api_key: Option<String>,
    req: SpeechRequest,
) -> Result<SpeechAudio, ProviderError> {
    if req.text.trim().is_empty() {
        return Err(ProviderError::BadRequest("empty text".into()));
    }
    let mut body = json!({
        "model": req.model.unwrap_or_else(|| DEFAULT_TTS_MODEL.to_owned()),
        "input": req.text,
        "voice": req.voice,
        "response_format": "mp3",
    });
    if let Some(style) = req.style.filter(|s| !s.trim().is_empty()) {
        body["instructions"] = Value::String(style);
    }
    let mut rb = cfg.client.post(cfg.url("audio/speech")).timeout(AUDIO_TIMEOUT).json(&body);
    rb = cfg.with_headers(rb);
    if let Some(k) = api_key {
        rb = rb.bearer_auth(k);
    }
    let resp = http::send(rb).await?;
    let mime = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(sanitize_mime)
        .filter(|m| m.starts_with("audio/"))
        .unwrap_or_else(|| "audio/mpeg".to_owned());
    let bytes = resp.bytes().await.map_err(http::map_reqwest_error)?.to_vec();
    if bytes.is_empty() {
        return Err(ProviderError::Server("speech response is empty".into()));
    }
    Ok(SpeechAudio { bytes, mime_type: mime })
}

fn uuid_like() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let n = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("{n:x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multipart_layout() {
        let (ct, body) = multipart_body("B", &[("model", "whisper-1")], ("a.wav", "audio/wav", b"RIFF"));
        assert_eq!(ct, "multipart/form-data; boundary=B");
        let s = String::from_utf8(body).unwrap();
        assert!(s.starts_with("--B\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\nwhisper-1\r\n"));
        assert!(s.contains("name=\"file\"; filename=\"a.wav\"\r\nContent-Type: audio/wav\r\n\r\nRIFF\r\n--B--\r\n"));
    }

    #[test]
    fn mime_and_extension() {
        assert_eq!(extension_for("audio/webm;codecs=opus"), "webm");
        assert_eq!(extension_for("audio/wav"), "wav");
        assert_eq!(extension_for("weird"), "wav");
        assert_eq!(sanitize_mime("audio/webm; codecs=opus"), "audio/webm");
        assert_eq!(sanitize_mime("audio/x\"evil"), "audio/wav");
    }
}
