//! A resident `whisper-server` (ADR-0057): the whisper.cpp model stays loaded
//! (on the GPU with the CUDA runtime) between utterances, so a command costs
//! one decode instead of process start + model load (≈0.3–3 s per call with
//! `whisper-cli`).
//!
//! - One server at a time, keyed by runtime + model + VAD model; a settings
//!   change restarts it. It listens on `127.0.0.1` on a free port only.
//! - Stopped after [`IDLE`] without requests and when the service is dropped
//!   (`kill_on_drop`). A server orphaned by a crash is killed on the next start
//!   (its pid is kept in `voice/models/.tmp/whisper-server.pid`).
//! - Any failure makes the caller fall back to `whisper-cli`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::gate::Segment;
use crate::VoiceError;

/// A resident server is stopped after this long without requests.
pub const IDLE: Duration = Duration::from_secs(15 * 60);
/// Model load timeout (large models on a cold disk / first CUDA init).
const START_TIMEOUT: Duration = Duration::from_secs(120);
/// One request.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(90);
/// Speech padding around VAD segments: with the default 30 ms the first
/// syllable is clipped («Какая погода…» → «Пока, пока…»).
pub const VAD_PAD_MS: &str = "200";

/// What a server was started with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerKey {
    /// Runtime directory (holds `whisper-server.exe`).
    pub runtime: PathBuf,
    /// Model file.
    pub model: PathBuf,
    /// Silero VAD model, when installed.
    pub vad: Option<PathBuf>,
    /// Use the GPU.
    pub gpu: bool,
}

struct Running {
    key: ServerKey,
    port: u16,
    child: tokio::process::Child,
    last_used: Instant,
}

struct Inner {
    running: tokio::sync::Mutex<Option<Running>>,
    http: reqwest::Client,
    pid_file: PathBuf,
}

/// The resident whisper server (cheap to clone).
#[derive(Clone)]
pub struct WhisperServers(Arc<Inner>);

impl std::fmt::Debug for WhisperServers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WhisperServers").finish_non_exhaustive()
    }
}

/// Per-request options.
#[derive(Debug, Clone, Default)]
pub struct Request<'a> {
    /// Language (`ru`, `en`, `auto`).
    pub language: &'a str,
    /// Initial prompt.
    pub prompt: Option<&'a str>,
    /// Run the Silero VAD (needs `ServerKey::vad`).
    pub vad: bool,
}

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    }
}

fn free_port() -> Result<u16, VoiceError> {
    let l = std::net::TcpListener::bind(("127.0.0.1", 0)).map_err(|e| VoiceError::Failed(e.to_string()))?;
    l.local_addr().map(|a| a.port()).map_err(|e| VoiceError::Failed(e.to_string()))
}

impl WhisperServers {
    /// A manager keeping its pid file under `tmp_dir`.
    #[must_use]
    pub fn new(tmp_dir: &Path) -> Self {
        let http =
            reqwest::Client::builder().no_proxy().connect_timeout(Duration::from_secs(5)).build().unwrap_or_default();
        let inner = Arc::new(Inner {
            running: tokio::sync::Mutex::new(None),
            http,
            pid_file: tmp_dir.join("whisper-server.pid"),
        });
        Self(inner)
    }

    /// Whether a server with exactly this key is running.
    pub async fn is_running(&self, key: &ServerKey) -> bool {
        self.0.running.lock().await.as_ref().is_some_and(|r| &r.key == key)
    }

    /// Stops the server (if any).
    pub async fn stop(&self) {
        if let Some(mut r) = self.0.running.lock().await.take() {
            let _ = r.child.kill().await;
            let _ = std::fs::remove_file(&self.0.pid_file);
        }
    }

    /// Starts the server for `key` unless it already runs (model preload).
    pub async fn ensure(&self, key: &ServerKey) -> Result<(), VoiceError> {
        let mut g = self.0.running.lock().await;
        self.ensure_locked(&mut g, key).await.map(|_| ())
    }

    async fn ensure_locked(&self, g: &mut Option<Running>, key: &ServerKey) -> Result<u16, VoiceError> {
        if let Some(r) = g.as_mut() {
            if &r.key == key && matches!(r.child.try_wait(), Ok(None)) {
                r.last_used = Instant::now();
                return Ok(r.port);
            }
        }
        if let Some(mut old) = g.take() {
            let _ = old.child.kill().await;
        }
        self.kill_orphan();
        let running = self.spawn(key).await?;
        let port = running.port;
        *g = Some(running);
        self.reaper();
        Ok(port)
    }

    /// Kills a server left behind by a crashed run.
    fn kill_orphan(&self) {
        let Ok(raw) = std::fs::read_to_string(&self.0.pid_file) else { return };
        let _ = std::fs::remove_file(&self.0.pid_file);
        let Ok(pid) = raw.trim().parse::<u32>() else { return };
        let ours = orkestr_platform::process::process_exe(pid)
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()))
            .is_some_and(|n| n.starts_with("whisper-server"));
        if ours {
            tracing::info!(pid, "killing an orphaned whisper-server");
            orkestr_platform::process::kill_pids(&[pid]);
        }
    }

    async fn spawn(&self, key: &ServerKey) -> Result<Running, VoiceError> {
        let program = key.runtime.join(exe("whisper-server"));
        if !program.is_file() {
            return Err(VoiceError::Unavailable("this whisper.cpp runtime has no whisper-server".into()));
        }
        let port = free_port()?;
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get().clamp(1, 8));
        let mut args: Vec<String> = vec![
            "-m".into(),
            key.model.display().to_string(),
            "--host".into(),
            "127.0.0.1".into(),
            "--port".into(),
            port.to_string(),
            "-t".into(),
            threads.to_string(),
        ];
        if !key.gpu {
            args.push("-ng".into());
        }
        if let Some(vad) = &key.vad {
            args.extend(["-vm".into(), vad.display().to_string(), "-vp".into(), VAD_PAD_MS.into()]);
        }
        let mut cmd = tokio::process::Command::new(&program);
        cmd.args(&args)
            .current_dir(&key.runtime)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        orkestr_platform::process::isolate_command(&mut cmd);
        let started = Instant::now();
        let mut child = cmd.spawn().map_err(|e| VoiceError::Failed(format!("could not start whisper-server: {e}")))?;
        if let Some(pid) = child.id() {
            let _ = std::fs::create_dir_all(self.0.pid_file.parent().unwrap_or(Path::new(".")));
            let _ = std::fs::write(&self.0.pid_file, pid.to_string());
        }
        let url = format!("http://127.0.0.1:{port}/");
        loop {
            if let Ok(Some(status)) = child.try_wait() {
                return Err(VoiceError::Failed(format!("whisper-server exited while loading ({status})")));
            }
            if self.0.http.get(&url).timeout(Duration::from_secs(2)).send().await.is_ok() {
                break;
            }
            if started.elapsed() > START_TIMEOUT {
                let _ = child.kill().await;
                return Err(VoiceError::Failed("whisper-server did not start in time".into()));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        tracing::info!(model = %key.model.display(), gpu = key.gpu, ms = started.elapsed().as_millis(), "whisper-server ready");
        Ok(Running { key: key.clone(), port, child, last_used: Instant::now() })
    }

    /// Stops an idle server in the background.
    fn reaper(&self) {
        let weak: Weak<Inner> = Arc::downgrade(&self.0);
        let Ok(rt) = tokio::runtime::Handle::try_current() else { return };
        rt.spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                let Some(inner) = weak.upgrade() else { return };
                let mut g = inner.running.lock().await;
                match g.as_ref() {
                    None => return,
                    Some(r) if r.last_used.elapsed() >= IDLE => {
                        if let Some(mut r) = g.take() {
                            let _ = r.child.kill().await;
                            let _ = std::fs::remove_file(&inner.pid_file);
                            tracing::info!("whisper-server stopped after idling");
                        }
                        return;
                    }
                    Some(_) => {}
                }
            }
        });
    }

    /// Transcribes a 16 kHz WAV.
    pub async fn transcribe(&self, key: &ServerKey, wav: &[u8], req: &Request<'_>) -> Result<Vec<Segment>, VoiceError> {
        // The lock serializes requests (the server decodes one at a time anyway).
        let mut g = self.0.running.lock().await;
        let port = self.ensure_locked(&mut g, key).await?;
        let boundary = format!("orkestr-whisper-{}", std::process::id());
        let mut body = Vec::with_capacity(wav.len() + 1024);
        let mut field = |name: &str, value: &str| {
            body.extend_from_slice(
                format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n")
                    .as_bytes(),
            );
        };
        field("response_format", "verbose_json");
        field("temperature", "0.0");
        field("language", req.language);
        field("no_language_probabilities", "true");
        if let Some(p) = req.prompt.filter(|p| !p.is_empty()) {
            field("prompt", p);
        }
        if req.vad && key.vad.is_some() {
            field("vad", "true");
            field("vad_speech_pad_ms", VAD_PAD_MS);
        }
        body.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"speech.wav\"\r\nContent-Type: audio/wav\r\n\r\n").as_bytes(),
        );
        body.extend_from_slice(wav);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let res = self
            .0
            .http
            .post(format!("http://127.0.0.1:{port}/inference"))
            .header(reqwest::header::CONTENT_TYPE, format!("multipart/form-data; boundary={boundary}"))
            .timeout(REQUEST_TIMEOUT)
            .body(body)
            .send()
            .await;
        let res = match res {
            Ok(r) if r.status().is_success() => r,
            Ok(r) => {
                let status = r.status();
                return Err(VoiceError::Failed(format!("whisper-server: HTTP {status}")));
            }
            Err(e) => {
                // a dead / hung server is restarted on the next call
                if let Some(mut r) = g.take() {
                    let _ = r.child.kill().await;
                }
                return Err(VoiceError::Failed(format!("whisper-server: {e}")));
            }
        };
        if let Some(r) = g.as_mut() {
            r.last_used = Instant::now();
        }
        let v: Value = res.json().await.map_err(|e| VoiceError::Failed(format!("whisper-server: {e}")))?;
        Ok(parse_verbose_json(&v))
    }
}

/// Segments of a `verbose_json` answer (`segments[].{text, avg_logprob, no_speech_prob, tokens}`).
#[must_use]
pub fn parse_verbose_json(v: &Value) -> Vec<Segment> {
    let f = |s: &Value, k: &str| {
        #[allow(clippy::cast_possible_truncation)]
        s.get(k).and_then(Value::as_f64).map(|x| x as f32)
    };
    match v.get("segments").and_then(Value::as_array) {
        Some(segs) => segs
            .iter()
            .map(|s| Segment {
                text: s.get("text").and_then(Value::as_str).unwrap_or_default().to_owned(),
                avg_logprob: f(s, "avg_logprob").unwrap_or(0.0),
                no_speech_prob: f(s, "no_speech_prob").unwrap_or(0.0),
                tokens: s.get("tokens").and_then(Value::as_array).map_or(1, Vec::len),
            })
            .collect(),
        None => {
            let text = v.get("text").and_then(Value::as_str).unwrap_or_default();
            vec![Segment { text: text.to_owned(), avg_logprob: 0.0, no_speech_prob: 0.0, tokens: 1 }]
        }
    }
}

/// Segments of `whisper-cli -ojf` output (`transcription[].{text, tokens[].p}`):
/// the mean log-probability of the text tokens (special `[_…]` tokens skipped).
#[must_use]
pub fn parse_cli_json(v: &Value) -> Vec<Segment> {
    v.get("transcription")
        .and_then(Value::as_array)
        .map(|segs| {
            segs.iter()
                .map(|s| {
                    let probs: Vec<f64> = s
                        .get("tokens")
                        .and_then(Value::as_array)
                        .map(|t| {
                            t.iter()
                                .filter(|t| {
                                    !t.get("text").and_then(Value::as_str).unwrap_or_default().starts_with("[_")
                                })
                                .filter_map(|t| t.get("p").and_then(Value::as_f64))
                                .collect()
                        })
                        .unwrap_or_default();
                    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
                    let avg = if probs.is_empty() {
                        0.0
                    } else {
                        (probs.iter().map(|p| p.max(1e-6).ln()).sum::<f64>() / probs.len() as f64) as f32
                    };
                    Segment {
                        text: s.get("text").and_then(Value::as_str).unwrap_or_default().to_owned(),
                        avg_logprob: avg,
                        no_speech_prob: 0.0,
                        tokens: probs.len().max(1),
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_server_and_cli_json() {
        let v = serde_json::json!({"text": " открой ютуб", "segments": [
            {"text": " открой ютуб", "avg_logprob": -0.2, "no_speech_prob": 0.01, "tokens": [1, 2, 3]}
        ]});
        let s = parse_verbose_json(&v);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].tokens, 3);
        assert!((s[0].avg_logprob + 0.2).abs() < 1e-6);
        let c = serde_json::json!({"transcription": [{"text": " да", "tokens": [
            {"text": "[_BEG_]", "p": 0.1}, {"text": " да", "p": 0.9}
        ]}]});
        let s = parse_cli_json(&c);
        assert!((s[0].avg_logprob - 0.9f32.ln()).abs() < 1e-4);
        assert_eq!(s[0].tokens, 1);
    }
}
