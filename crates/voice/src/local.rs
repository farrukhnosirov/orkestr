//! Offline engines: whisper.cpp, Vosk, sherpa-onnx and Windows speech
//! recognition; Piper, Windows OneCore voices and Silero synthesis.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use orkestr_protocol::methods::voice::VoiceEngineSettings;
use parking_lot::Mutex;
use serde::Deserialize;

use crate::audio::Pcm;
use crate::models::ModelManager;
use crate::proc;
use crate::{Clip, VoiceError};

const STT_TIMEOUT: Duration = Duration::from_secs(180);
const TTS_TIMEOUT: Duration = Duration::from_secs(120);

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    }
}

fn need(models: &ModelManager, id: &str) -> Result<PathBuf, VoiceError> {
    models.installed_dir(id).ok_or_else(|| VoiceError::NotInstalled(id.to_owned()))
}

fn find_file(dir: &Path, pred: impl Fn(&str) -> bool) -> Option<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.file_name().and_then(|n| n.to_str()).is_some_and(&pred))
        .collect();
    v.sort();
    v.into_iter().next()
}

/// A unique temp file under `voice/models/.tmp` (deleted by the guard).
pub(crate) struct TempFile(pub PathBuf);
impl TempFile {
    pub(crate) fn new(models: &ModelManager, ext: &str) -> Result<Self, VoiceError> {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = models.root().join(".tmp");
        std::fs::create_dir_all(&dir).map_err(|e| VoiceError::Failed(e.to_string()))?;
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(Self(dir.join(format!("speech-{}-{n}.{ext}", std::process::id()))))
    }
}
impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn lang_or_auto(language: Option<&str>) -> String {
    language
        .map(orkestr_protocol::methods::voice::primary_language)
        .filter(|l| (2..=3).contains(&l.len()) && l.chars().all(|c| c.is_ascii_lowercase()))
        .unwrap_or_else(|| "auto".into())
}

// --- whisper.cpp -----------------------------------------------------------

/// Transcribes with whisper-cli.
pub async fn whisper(
    models: &ModelManager,
    s: &VoiceEngineSettings,
    pcm: &Pcm,
    language: Option<&str>,
) -> Result<String, VoiceError> {
    let runtime = if s.whisper_gpu && models.is_installed("whisper-runtime-cuda") {
        need(models, "whisper-runtime-cuda")?
    } else {
        need(models, "whisper-runtime")?
    };
    let model_dir = need(models, &s.whisper_model)?;
    let model = find_file(&model_dir, |n| n.ends_with(".bin"))
        .ok_or_else(|| VoiceError::NotInstalled(s.whisper_model.clone()))?;
    let wav = TempFile::new(models, "wav")?;
    std::fs::write(&wav.0, pcm.to_wav()).map_err(|e| VoiceError::Failed(e.to_string()))?;
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get().clamp(1, 8));
    let args: Vec<String> = vec![
        "-m".into(),
        model.display().to_string(),
        "-f".into(),
        wav.0.display().to_string(),
        "-l".into(),
        lang_or_auto(language),
        "-nt".into(),
        "-np".into(),
        "-t".into(),
        threads.to_string(),
    ];
    let out = proc::run(&runtime.join(exe("whisper-cli")), &args, &[], None, STT_TIMEOUT).await?;
    Ok(out.stdout.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" "))
}

// --- Vosk ------------------------------------------------------------------

/// Cached Vosk library + model.
#[derive(Default)]
pub struct VoskCache {
    lib: Mutex<Option<orkestr_platform::vosk::VoskLibrary>>,
    model: Mutex<Option<Arc<orkestr_platform::vosk::VoskModel>>>,
}

/// Transcribes with libvosk (in process, model cached).
pub async fn vosk(
    cache: &Arc<VoskCache>,
    models: &ModelManager,
    s: &VoiceEngineSettings,
    pcm: &Pcm,
) -> Result<String, VoiceError> {
    let runtime = need(models, "vosk-runtime")?;
    let model_dir = need(models, &s.vosk_model)?;
    let cache = Arc::clone(cache);
    let samples = pcm.samples.clone();
    tokio::task::spawn_blocking(move || {
        let native = |e: orkestr_platform::PlatformError| VoiceError::Failed(e.to_string());
        let lib = {
            let mut g = cache.lib.lock();
            if g.is_none() {
                *g = Some(orkestr_platform::vosk::VoskLibrary::load(&runtime).map_err(native)?);
            }
            g.clone().ok_or_else(|| VoiceError::Failed("vosk".into()))?
        };
        let model = {
            let mut g = cache.model.lock();
            match g.as_ref() {
                Some(m) if m.path() == model_dir => Arc::clone(m),
                _ => {
                    let m = Arc::new(lib.model(&model_dir).map_err(native)?);
                    *g = Some(Arc::clone(&m));
                    m
                }
            }
        };
        model.recognize(&samples, crate::audio::SAMPLE_RATE).map_err(native)
    })
    .await
    .map_err(|e| VoiceError::Failed(e.to_string()))?
}

// --- sherpa-onnx -----------------------------------------------------------

/// Transcribes with the sherpa-onnx CLI (streaming transducer models).
pub async fn sherpa(models: &ModelManager, s: &VoiceEngineSettings, pcm: &Pcm) -> Result<String, VoiceError> {
    let runtime = need(models, "sherpa-runtime")?;
    let dir = need(models, &s.sherpa_model)?;
    let pick = |prefix: &str| {
        find_file(&dir, |n| n.starts_with(prefix) && n.ends_with(".int8.onnx"))
            .or_else(|| find_file(&dir, |n| n.starts_with(prefix) && n.ends_with(".onnx")))
            .ok_or_else(|| VoiceError::Failed(format!("{}: no {prefix}*.onnx", s.sherpa_model)))
    };
    let (enc, dec, join) = (pick("encoder")?, pick("decoder")?, pick("joiner")?);
    let wav = TempFile::new(models, "wav")?;
    std::fs::write(&wav.0, pcm.to_wav()).map_err(|e| VoiceError::Failed(e.to_string()))?;
    let args = vec![
        format!("--tokens={}", dir.join("tokens.txt").display()),
        format!("--encoder={}", enc.display()),
        format!("--decoder={}", dec.display()),
        format!("--joiner={}", join.display()),
        wav.0.display().to_string(),
    ];
    let out = proc::run(&runtime.join("bin").join(exe("sherpa-onnx")), &args, &[], None, STT_TIMEOUT).await?;
    // The result is a JSON line `{ "text": … }` (stdout or stderr depending on version).
    let all = format!("{}\n{}", out.stdout, out.stderr);
    Ok(all
        .lines()
        .rev()
        .filter(|l| l.trim_start().starts_with('{'))
        .find_map(|l| serde_json::from_str::<serde_json::Value>(l.trim()).ok())
        .and_then(|v| v.get("text").and_then(|t| t.as_str()).map(|t| t.trim().to_owned()))
        .unwrap_or_default())
}

// --- Windows speech (PowerShell) ------------------------------------------

/// What Windows offers on this machine.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct WindowsSpeech {
    /// `System.Speech` recognizer cultures (`en-US`, …).
    #[serde(default)]
    pub recognizers: Vec<String>,
    /// OneCore voices.
    #[serde(default)]
    pub voices: Vec<WindowsVoice>,
}

/// A OneCore voice.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WindowsVoice {
    /// Display name (`Microsoft Pavel`).
    pub name: String,
    /// Language (`ru-RU`).
    pub language: String,
}

const DETECT_PS: &str = r"
$ErrorActionPreference = 'SilentlyContinue'
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
Add-Type -AssemblyName System.Speech
$rec = @([System.Speech.Recognition.SpeechRecognitionEngine]::InstalledRecognizers() | ForEach-Object { $_.Culture.Name })
$null = [Windows.Media.SpeechSynthesis.SpeechSynthesizer, Windows.Media.SpeechSynthesis, ContentType = WindowsRuntime]
$voices = @([Windows.Media.SpeechSynthesis.SpeechSynthesizer]::AllVoices | ForEach-Object { @{ name = $_.DisplayName; language = $_.Language } })
@{ recognizers = $rec; voices = $voices } | ConvertTo-Json -Compress -Depth 4
";

/// Detects Windows recognizers and voices (≈1 s; cache the result).
pub async fn detect_windows() -> WindowsSpeech {
    if !cfg!(windows) {
        return WindowsSpeech::default();
    }
    match proc::run(&proc::powershell(), &proc::powershell_args(DETECT_PS), &[], None, Duration::from_secs(30)).await {
        Ok(out) => serde_json::from_str(out.stdout.trim()).unwrap_or_else(|e| {
            tracing::warn!(error = %e, "could not parse Windows speech detection");
            WindowsSpeech::default()
        }),
        Err(e) => {
            tracing::warn!(error = %e, "Windows speech detection failed");
            WindowsSpeech::default()
        }
    }
}

const RECOGNIZE_PS: &str = r"
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
Add-Type -AssemblyName System.Speech
$culture = $env:ORKESTR_VOICE_CULTURE
$info = [System.Speech.Recognition.SpeechRecognitionEngine]::InstalledRecognizers() | Where-Object { $_.Culture.Name -eq $culture } | Select-Object -First 1
if (-not $info) { [Console]::Error.WriteLine('no recognizer for ' + $culture); exit 3 }
$engine = New-Object System.Speech.Recognition.SpeechRecognitionEngine($info)
$engine.LoadGrammar((New-Object System.Speech.Recognition.DictationGrammar))
$engine.SetInputToWaveFile($env:ORKESTR_VOICE_IN)
$parts = @()
while ($true) { $r = $engine.Recognize(); if (-not $r) { break }; $parts += $r.Text }
$engine.Dispose()
[Console]::Out.Write(($parts -join ' '))
";

/// Picks the recognizer culture for `language` from the installed ones.
#[must_use]
pub fn windows_culture(installed: &[String], language: Option<&str>) -> Option<String> {
    let want = language.unwrap_or("en-US");
    let primary = orkestr_protocol::methods::voice::primary_language(want);
    installed
        .iter()
        .find(|c| c.eq_ignore_ascii_case(want))
        .or_else(|| installed.iter().find(|c| orkestr_protocol::methods::voice::primary_language(c) == primary))
        .cloned()
}

/// Transcribes with `System.Speech` dictation.
pub async fn windows_stt(
    models: &ModelManager,
    detected: &WindowsSpeech,
    pcm: &Pcm,
    language: Option<&str>,
) -> Result<String, VoiceError> {
    if !cfg!(windows) {
        return Err(VoiceError::Unavailable("Windows speech recognition needs Windows".into()));
    }
    let culture = windows_culture(&detected.recognizers, language).ok_or_else(|| {
        VoiceError::Unavailable(format!(
            "no Windows speech recognizer for {} (installed: {}); add the language's speech pack in Windows Settings",
            language.unwrap_or("this language"),
            if detected.recognizers.is_empty() { "none".to_owned() } else { detected.recognizers.join(", ") }
        ))
    })?;
    let wav = TempFile::new(models, "wav")?;
    std::fs::write(&wav.0, pcm.to_wav()).map_err(|e| VoiceError::Failed(e.to_string()))?;
    let env = [("ORKESTR_VOICE_CULTURE", culture), ("ORKESTR_VOICE_IN", wav.0.display().to_string())];
    let out = proc::run(&proc::powershell(), &proc::powershell_args(RECOGNIZE_PS), &env, None, STT_TIMEOUT).await?;
    Ok(out.stdout.trim().to_owned())
}

const SYNTH_PS: &str = r"
$ErrorActionPreference = 'Stop'
[Console]::InputEncoding = [System.Text.Encoding]::UTF8
$text = [Console]::In.ReadToEnd()
Add-Type -AssemblyName System.Runtime.WindowsRuntime
$null = [Windows.Media.SpeechSynthesis.SpeechSynthesizer, Windows.Media.SpeechSynthesis, ContentType = WindowsRuntime]
$null = [Windows.Storage.Streams.DataReader, Windows.Storage.Streams, ContentType = WindowsRuntime]
$asTask = ([System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object { $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1' })[0]
function Await($op, [Type]$t) { $task = $asTask.MakeGenericMethod($t).Invoke($null, @($op)); $task.Wait(-1) | Out-Null; $task.Result }
$s = New-Object Windows.Media.SpeechSynthesis.SpeechSynthesizer
$all = [Windows.Media.SpeechSynthesis.SpeechSynthesizer]::AllVoices
$v = $null
if ($env:ORKESTR_VOICE_NAME) { $n = $env:ORKESTR_VOICE_NAME; $v = $all | Where-Object { $_.DisplayName -eq $n -or $n.StartsWith($_.DisplayName + ' ') } | Select-Object -First 1 }
if (-not $v -and $env:ORKESTR_VOICE_LANG) { $v = $all | Where-Object { $_.Language -eq $env:ORKESTR_VOICE_LANG } | Select-Object -First 1 }
if (-not $v -and $env:ORKESTR_VOICE_LANG) { $p = $env:ORKESTR_VOICE_LANG.Split('-')[0]; $v = $all | Where-Object { $_.Language.Split('-')[0] -eq $p } | Select-Object -First 1 }
if ($v) { $s.Voice = $v }
try { $s.Options.SpeakingRate = [double]$env:ORKESTR_VOICE_RATE } catch {}
$stream = Await ($s.SynthesizeTextToStreamAsync($text)) ([Windows.Media.SpeechSynthesis.SpeechSynthesisStream])
$size = [uint32]$stream.Size
$reader = New-Object Windows.Storage.Streams.DataReader($stream.GetInputStreamAt(0))
$null = Await ($reader.LoadAsync($size)) ([uint32])
$bytes = New-Object byte[] $size
$reader.ReadBytes($bytes)
[System.IO.File]::WriteAllBytes($env:ORKESTR_VOICE_OUT, $bytes)
";

/// Renders `text` with a OneCore voice to WAV.
pub async fn windows_tts(
    models: &ModelManager,
    voice: Option<&str>,
    language: &str,
    rate_pct: u16,
    text: &str,
) -> Result<Clip, VoiceError> {
    if !cfg!(windows) {
        return Err(VoiceError::Unavailable("Windows voices need Windows".into()));
    }
    let out = TempFile::new(models, "wav")?;
    let env = [
        ("ORKESTR_VOICE_NAME", voice.unwrap_or_default().to_owned()),
        ("ORKESTR_VOICE_LANG", language.to_owned()),
        ("ORKESTR_VOICE_RATE", format!("{:.2}", f64::from(rate_pct.clamp(50, 200)) / 100.0)),
        ("ORKESTR_VOICE_OUT", out.0.display().to_string()),
    ];
    proc::run(&proc::powershell(), &proc::powershell_args(SYNTH_PS), &env, Some(text.as_bytes()), TTS_TIMEOUT).await?;
    let bytes = std::fs::read(&out.0).map_err(|e| VoiceError::Failed(e.to_string()))?;
    Ok(Clip { bytes, mime_type: "audio/wav".into() })
}

// --- Piper -----------------------------------------------------------------

/// Renders `text` with a Piper voice to WAV.
pub async fn piper(models: &ModelManager, voice_id: &str, rate_pct: u16, text: &str) -> Result<Clip, VoiceError> {
    let runtime = need(models, "piper-runtime")?;
    let dir = need(models, voice_id)?;
    let model = find_file(&dir, |n| n.ends_with(".onnx")).ok_or_else(|| VoiceError::NotInstalled(voice_id.into()))?;
    let out = TempFile::new(models, "wav")?;
    let length_scale = 100.0 / f64::from(rate_pct.clamp(50, 200));
    let args = vec![
        "-m".into(),
        model.display().to_string(),
        "-f".into(),
        out.0.display().to_string(),
        "--length_scale".into(),
        format!("{length_scale:.2}"),
    ];
    // Piper reads one utterance per line.
    let line = text.replace(['\r', '\n'], " ");
    proc::run(&runtime.join(exe("piper")), &args, &[], Some(line.as_bytes()), TTS_TIMEOUT).await?;
    let bytes = std::fs::read(&out.0).map_err(|e| VoiceError::Failed(e.to_string()))?;
    Ok(Clip { bytes, mime_type: "audio/wav".into() })
}

// --- Silero ----------------------------------------------------------------

/// Env var naming a Python interpreter that has `torch` installed (Silero).
pub const SILERO_PYTHON_ENV: &str = "ORKESTR_SILERO_PYTHON";

const SILERO_PY: &str = r"
import os, sys, torch
text = sys.stdin.buffer.read().decode('utf-8')
model, _ = torch.hub.load('snakers4/silero-models', 'silero_tts', language='ru', speaker='v4_ru', trust_repo=True)
model.save_wav(text=text, speaker=os.environ['ORKESTR_VOICE_NAME'], sample_rate=48000, audio_path=os.environ['ORKESTR_VOICE_OUT'])
";

/// The configured Silero Python, if any.
#[must_use]
pub fn silero_python() -> Option<PathBuf> {
    std::env::var_os(SILERO_PYTHON_ENV).map(PathBuf::from).filter(|p| p.is_file())
}

/// Renders `text` with Silero v4 (Russian) through the configured Python.
pub async fn silero(models: &ModelManager, speaker: &str, text: &str) -> Result<Clip, VoiceError> {
    let python = silero_python().ok_or_else(|| {
        VoiceError::Unavailable(format!(
            "Silero needs Python with PyTorch; set {SILERO_PYTHON_ENV} to that python.exe (see ADR-0046)"
        ))
    })?;
    let out = TempFile::new(models, "wav")?;
    let env = [("ORKESTR_VOICE_NAME", speaker.to_owned()), ("ORKESTR_VOICE_OUT", out.0.display().to_string())];
    let args = vec!["-I".to_owned(), "-c".to_owned(), SILERO_PY.to_owned()];
    proc::run(&python, &args, &env, Some(text.as_bytes()), Duration::from_secs(300)).await?;
    let bytes = std::fs::read(&out.0).map_err(|e| VoiceError::Failed(e.to_string()))?;
    Ok(Clip { bytes, mime_type: "audio/wav".into() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn culture_matching() {
        let installed = vec!["en-US".to_owned(), "ru-RU".to_owned()];
        assert_eq!(windows_culture(&installed, Some("ru")).as_deref(), Some("ru-RU"));
        assert_eq!(windows_culture(&installed, Some("en-GB")).as_deref(), Some("en-US"));
        assert_eq!(windows_culture(&["en-US".to_owned()], Some("ru-RU")), None);
        assert_eq!(lang_or_auto(Some("ru-RU")), "ru");
        assert_eq!(lang_or_auto(None), "auto");
    }
}
