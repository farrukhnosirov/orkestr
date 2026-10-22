//! Paid cloud engines (ADR-0046). Request shapes follow each vendor's
//! public REST API; they are covered by tests against a local mock server
//! ("request-shape verified"). Keys come from the secret vault and are only
//! ever sent in headers (never in URLs, never logged).

use std::time::Duration;

use base64::Engine as _;
use orkestr_protocol::methods::voice::VoiceEngineSettings;
use serde_json::{json, Value};

use crate::audio::Pcm;
use crate::{Clip, VoiceError};

const TIMEOUT: Duration = Duration::from_secs(60);

/// Service base URLs (overridable for tests). `{region}` is replaced with the Azure region.
#[derive(Debug, Clone)]
pub struct Endpoints {
    /// OpenAI API base (`…/v1`).
    pub openai: String,
    /// Deepgram base.
    pub deepgram: String,
    /// Azure STT base.
    pub azure_stt: String,
    /// Azure TTS base.
    pub azure_tts: String,
    /// Google STT base.
    pub google_stt: String,
    /// Google TTS base.
    pub google_tts: String,
    /// Yandex STT base.
    pub yandex_stt: String,
    /// Yandex TTS base.
    pub yandex_tts: String,
    /// ElevenLabs base.
    pub elevenlabs: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            openai: "https://api.openai.com/v1".into(),
            deepgram: "https://api.deepgram.com".into(),
            azure_stt: "https://{region}.stt.speech.microsoft.com".into(),
            azure_tts: "https://{region}.tts.speech.microsoft.com".into(),
            google_stt: "https://speech.googleapis.com".into(),
            google_tts: "https://texttospeech.googleapis.com".into(),
            yandex_stt: "https://stt.api.cloud.yandex.net".into(),
            yandex_tts: "https://tts.api.cloud.yandex.net".into(),
            elevenlabs: "https://api.elevenlabs.io".into(),
        }
    }
}

/// BCP-47 locale for services that want one (`ru` → `ru-RU`).
#[must_use]
pub fn locale(language: Option<&str>) -> String {
    match language.unwrap_or("ru") {
        l if l.contains('-') => l.to_owned(),
        "ru" => "ru-RU".into(),
        "en" => "en-US".into(),
        "uk" => "uk-UA".into(),
        "de" => "de-DE".into(),
        l => format!("{l}-{}", l.to_ascii_uppercase()),
    }
}

async fn send(rb: reqwest::RequestBuilder, what: &str) -> Result<reqwest::Response, VoiceError> {
    let resp = rb.timeout(TIMEOUT).send().await.map_err(|e| VoiceError::Network(format!("{what}: {e}")))?;
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let body: String = resp.text().await.unwrap_or_default().chars().take(300).collect();
    let msg = format!("{what}: HTTP {status}: {}", orkestr_security_free_redact(&body));
    Err(if status.as_u16() == 401 || status.as_u16() == 403 {
        VoiceError::NeedsKey(msg)
    } else {
        VoiceError::Network(msg)
    })
}

/// Error bodies never echo keys in practice; strip anything key-like anyway.
fn orkestr_security_free_redact(s: &str) -> String {
    s.split_whitespace()
        .map(|w| {
            if w.len() >= 32 && w.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)) {
                "[redacted]"
            } else {
                w
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

async fn json_body(resp: reqwest::Response, what: &str) -> Result<Value, VoiceError> {
    resp.json().await.map_err(|e| VoiceError::Network(format!("{what}: invalid JSON: {e}")))
}

async fn audio_clip(resp: reqwest::Response, default_mime: &str, what: &str) -> Result<Clip, VoiceError> {
    let mime = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|m| m.split(';').next().unwrap_or(m).trim().to_owned())
        .filter(|m| m.starts_with("audio/"))
        .unwrap_or_else(|| default_mime.to_owned());
    let bytes = resp.bytes().await.map_err(|e| VoiceError::Network(format!("{what}: {e}")))?.to_vec();
    if bytes.is_empty() {
        return Err(VoiceError::Network(format!("{what}: empty audio")));
    }
    Ok(Clip { bytes, mime_type: mime })
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

/// The cloud engines' client.
#[derive(Debug, Clone)]
pub struct Cloud {
    /// HTTP client.
    pub http: reqwest::Client,
    /// Endpoints.
    pub endpoints: Endpoints,
}

impl Cloud {
    /// OpenAI `POST /audio/transcriptions` (multipart).
    pub async fn openai_stt(
        &self,
        key: &str,
        s: &VoiceEngineSettings,
        wav: &[u8],
        language: Option<&str>,
    ) -> Result<String, VoiceError> {
        let boundary = format!("orkestr-voice-{}", std::process::id());
        let mut body = Vec::with_capacity(wav.len() + 512);
        let mut field = |name: &str, value: &str| {
            body.extend_from_slice(
                format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n")
                    .as_bytes(),
            );
        };
        field("model", &s.openai_stt_model);
        field("response_format", "json");
        if let Some(l) = language.map(orkestr_protocol::methods::voice::primary_language) {
            field("language", &l);
        }
        body.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"speech.wav\"\r\nContent-Type: audio/wav\r\n\r\n").as_bytes(),
        );
        body.extend_from_slice(wav);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let rb = self
            .http
            .post(format!("{}/audio/transcriptions", self.endpoints.openai))
            .bearer_auth(key)
            .header(reqwest::header::CONTENT_TYPE, format!("multipart/form-data; boundary={boundary}"))
            .body(body);
        let v = json_body(send(rb, "OpenAI transcription").await?, "OpenAI").await?;
        Ok(v.get("text").and_then(Value::as_str).unwrap_or_default().trim().to_owned())
    }

    /// OpenAI `POST /audio/speech` (MP3).
    pub async fn openai_tts(
        &self,
        key: &str,
        s: &VoiceEngineSettings,
        voice: &str,
        text: &str,
    ) -> Result<Clip, VoiceError> {
        let rb = self.http.post(format!("{}/audio/speech", self.endpoints.openai)).bearer_auth(key).json(&json!({
            "model": s.openai_tts_model, "input": text, "voice": voice, "response_format": "mp3"
        }));
        audio_clip(send(rb, "OpenAI speech").await?, "audio/mpeg", "OpenAI").await
    }

    /// Deepgram `POST /v1/listen` (pre-recorded, WAV body).
    pub async fn deepgram_stt(
        &self,
        key: &str,
        s: &VoiceEngineSettings,
        wav: &[u8],
        language: Option<&str>,
    ) -> Result<String, VoiceError> {
        let lang = language.map_or_else(|| "ru".to_owned(), orkestr_protocol::methods::voice::primary_language);
        let rb = self
            .http
            .post(format!("{}/v1/listen", self.endpoints.deepgram))
            .query(&[("model", s.deepgram_model.as_str()), ("language", lang.as_str()), ("smart_format", "true")])
            .header(reqwest::header::AUTHORIZATION, format!("Token {key}"))
            .header(reqwest::header::CONTENT_TYPE, "audio/wav")
            .body(wav.to_vec());
        let v = json_body(send(rb, "Deepgram").await?, "Deepgram").await?;
        Ok(v.pointer("/results/channels/0/alternatives/0/transcript")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_owned())
    }

    fn azure(&self, base: &str, s: &VoiceEngineSettings) -> Result<String, VoiceError> {
        if s.azure_region.is_empty() {
            return Err(VoiceError::NeedsKey("Azure Speech needs a region (Settings → Voice → engines)".into()));
        }
        Ok(base.replace("{region}", &s.azure_region))
    }

    /// Azure short-audio REST recognition.
    pub async fn azure_stt(
        &self,
        key: &str,
        s: &VoiceEngineSettings,
        wav: &[u8],
        language: Option<&str>,
    ) -> Result<String, VoiceError> {
        let base = self.azure(&self.endpoints.azure_stt, s)?;
        let loc = locale(language);
        let rb = self
            .http
            .post(format!("{base}/speech/recognition/conversation/cognitiveservices/v1"))
            .query(&[("language", loc.as_str()), ("format", "simple")])
            .header("Ocp-Apim-Subscription-Key", key)
            .header(reqwest::header::CONTENT_TYPE, "audio/wav; codecs=audio/pcm; samplerate=16000")
            .header(reqwest::header::ACCEPT, "application/json")
            .body(wav.to_vec());
        let v = json_body(send(rb, "Azure Speech").await?, "Azure").await?;
        match v.get("RecognitionStatus").and_then(Value::as_str) {
            Some("Success") => Ok(v.get("DisplayText").and_then(Value::as_str).unwrap_or_default().trim().to_owned()),
            Some("NoMatch" | "InitialSilenceTimeout") => Ok(String::new()),
            other => Err(VoiceError::Network(format!("Azure Speech: {}", other.unwrap_or("no status")))),
        }
    }

    /// Azure neural TTS (SSML → 24 kHz WAV).
    pub async fn azure_tts(
        &self,
        key: &str,
        s: &VoiceEngineSettings,
        voice: &str,
        language: Option<&str>,
        text: &str,
    ) -> Result<Clip, VoiceError> {
        let base = self.azure(&self.endpoints.azure_tts, s)?;
        let loc = locale(language);
        let ssml = format!(
            "<speak version='1.0' xml:lang='{}'><voice name='{}'>{}</voice></speak>",
            xml_escape(&loc),
            xml_escape(voice),
            xml_escape(text)
        );
        let rb = self
            .http
            .post(format!("{base}/cognitiveservices/v1"))
            .header("Ocp-Apim-Subscription-Key", key)
            .header(reqwest::header::CONTENT_TYPE, "application/ssml+xml")
            .header("X-Microsoft-OutputFormat", "riff-24khz-16bit-mono-pcm")
            .header(reqwest::header::USER_AGENT, "orkestr")
            .body(ssml);
        audio_clip(send(rb, "Azure TTS").await?, "audio/wav", "Azure").await
    }

    /// Google `speech:recognize` (v1, LINEAR16).
    pub async fn google_stt(&self, key: &str, pcm: &Pcm, language: Option<&str>) -> Result<String, VoiceError> {
        let body = json!({
            "config": { "encoding": "LINEAR16", "sampleRateHertz": crate::audio::SAMPLE_RATE, "languageCode": locale(language), "enableAutomaticPunctuation": true },
            "audio": { "content": base64::engine::general_purpose::STANDARD.encode(pcm.to_le_bytes()) }
        });
        let rb = self
            .http
            .post(format!("{}/v1/speech:recognize", self.endpoints.google_stt))
            .header("x-goog-api-key", key)
            .json(&body);
        let v = json_body(send(rb, "Google Speech-to-Text").await?, "Google").await?;
        let parts: Vec<&str> = v
            .get("results")
            .and_then(Value::as_array)
            .map(|r| r.iter().filter_map(|x| x.pointer("/alternatives/0/transcript").and_then(Value::as_str)).collect())
            .unwrap_or_default();
        Ok(parts.join(" ").trim().to_owned())
    }

    /// Google `text:synthesize` (LINEAR16 WAV).
    pub async fn google_tts(
        &self,
        key: &str,
        s: &VoiceEngineSettings,
        language: Option<&str>,
        text: &str,
    ) -> Result<Clip, VoiceError> {
        let mut voice = json!({ "languageCode": locale(language) });
        if !s.google_voice.is_empty() {
            voice["name"] = Value::String(s.google_voice.clone());
        }
        let rb = self
            .http
            .post(format!("{}/v1/text:synthesize", self.endpoints.google_tts))
            .header("x-goog-api-key", key)
            .json(
                &json!({ "input": { "text": text }, "voice": voice, "audioConfig": { "audioEncoding": "LINEAR16" } }),
            );
        let v = json_body(send(rb, "Google Text-to-Speech").await?, "Google").await?;
        let b64 = v.get("audioContent").and_then(Value::as_str).unwrap_or_default();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|_| VoiceError::Network("Google Text-to-Speech: invalid audioContent".into()))?;
        if bytes.is_empty() {
            return Err(VoiceError::Network("Google Text-to-Speech: empty audio".into()));
        }
        Ok(Clip { bytes, mime_type: "audio/wav".into() })
    }

    /// Yandex SpeechKit v1 short-audio recognition (raw LPCM).
    pub async fn yandex_stt(
        &self,
        key: &str,
        s: &VoiceEngineSettings,
        pcm: &Pcm,
        language: Option<&str>,
    ) -> Result<String, VoiceError> {
        let loc = locale(language);
        let mut q: Vec<(&str, String)> =
            vec![("lang", loc), ("format", "lpcm".into()), ("sampleRateHertz", crate::audio::SAMPLE_RATE.to_string())];
        if !s.yandex_folder_id.is_empty() {
            q.push(("folderId", s.yandex_folder_id.clone()));
        }
        let rb = self
            .http
            .post(format!("{}/speech/v1/stt:recognize", self.endpoints.yandex_stt))
            .query(&q)
            .header(reqwest::header::AUTHORIZATION, format!("Api-Key {key}"))
            .body(pcm.to_le_bytes());
        let v = json_body(send(rb, "Yandex SpeechKit").await?, "Yandex").await?;
        Ok(v.get("result").and_then(Value::as_str).unwrap_or_default().trim().to_owned())
    }

    /// Yandex SpeechKit v1 synthesis (form body → MP3).
    pub async fn yandex_tts(
        &self,
        key: &str,
        s: &VoiceEngineSettings,
        voice: &str,
        language: Option<&str>,
        text: &str,
    ) -> Result<Clip, VoiceError> {
        let mut form: Vec<(&str, String)> = vec![
            ("text", text.to_owned()),
            ("lang", locale(language)),
            ("voice", voice.to_owned()),
            ("format", "mp3".into()),
        ];
        if !s.yandex_folder_id.is_empty() {
            form.push(("folderId", s.yandex_folder_id.clone()));
        }
        let rb = self
            .http
            .post(format!("{}/speech/v1/tts:synthesize", self.endpoints.yandex_tts))
            .header(reqwest::header::AUTHORIZATION, format!("Api-Key {key}"))
            .form(&form);
        audio_clip(send(rb, "Yandex SpeechKit TTS").await?, "audio/mpeg", "Yandex").await
    }

    /// ElevenLabs `POST /v1/text-to-speech/{voice}` (MP3).
    pub async fn elevenlabs_tts(
        &self,
        key: &str,
        s: &VoiceEngineSettings,
        voice: &str,
        text: &str,
    ) -> Result<Clip, VoiceError> {
        if !orkestr_protocol::methods::voice::is_voice_id(voice, 64) {
            return Err(VoiceError::BadRequest("invalid ElevenLabs voice id".into()));
        }
        let rb = self
            .http
            .post(format!("{}/v1/text-to-speech/{voice}", self.endpoints.elevenlabs))
            .query(&[("output_format", "mp3_44100_128")])
            .header("xi-api-key", key)
            .header(reqwest::header::ACCEPT, "audio/mpeg")
            .json(&json!({ "text": text, "model_id": s.elevenlabs_model }));
        audio_clip(send(rb, "ElevenLabs").await?, "audio/mpeg", "ElevenLabs").await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locales_and_escaping() {
        assert_eq!(locale(Some("ru")), "ru-RU");
        assert_eq!(locale(Some("en-GB")), "en-GB");
        assert_eq!(locale(None), "ru-RU");
        assert_eq!(xml_escape("<a & 'b'>"), "&lt;a &amp; &apos;b&apos;&gt;");
        assert_eq!(orkestr_security_free_redact("bad key sk_0123456789abcdef0123456789abcdef"), "bad key [redacted]");
    }
}
