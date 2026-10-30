//! ADR-0057: background noise, music and TV must never become a command.
//!
//! Synthetic clips (generated here, deterministic): steady noise, hum,
//! clicks and silence are rejected by the speech-activity gate alone. With
//! `ORKESTR_VOICE_TEST_DIR` (whisper.cpp installed, see `tests/engines.rs`)
//! every clip — music included — goes through `transcribe_checked` and must
//! come back rejected, while the bundled command, also mixed with music,
//! is still recognized.

use orkestr_protocol::methods::voice::{SttEngine, VoiceEngineSettings};
use orkestr_voice::audio::{to_pcm16k, Pcm};
use orkestr_voice::{gate, SttHints, VoiceService};

const SR: usize = 16_000;
const BUNDLED: &[u8] = include_bytes!("data/ru_command.wav");

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        #[allow(clippy::cast_precision_loss)]
        let x = (self.0 >> 40) as f32 / (1u64 << 24) as f32;
        x * 2.0 - 1.0
    }
}

#[allow(clippy::cast_possible_truncation)]
fn pcm(v: &[f32]) -> Pcm {
    Pcm { samples: v.iter().map(|x| (x.clamp(-1.0, 1.0) * 32767.0) as i16).collect() }
}

fn white(secs: usize, amp: f32) -> Vec<f32> {
    let mut r = Rng(1);
    (0..secs * SR).map(|_| r.next() * amp).collect()
}

fn pink(secs: usize) -> Vec<f32> {
    let mut r = Rng(2);
    let mut b = [0.0f32; 3];
    (0..secs * SR)
        .map(|_| {
            let w = r.next();
            b[0] = 0.997 * b[0] + 0.029 * w;
            b[1] = 0.985 * b[1] + 0.032 * w;
            b[2] = 0.950 * b[2] + 0.048 * w;
            (b[0] + b[1] + b[2] + 0.02 * w) * 1.5
        })
        .collect()
}

#[allow(clippy::cast_precision_loss)]
fn hum(secs: usize) -> Vec<f32> {
    (0..secs * SR)
        .map(|i| {
            let t = i as f32 / SR as f32;
            0.1 * (2.0 * std::f32::consts::PI * 50.0 * t).sin() + 0.05 * (2.0 * std::f32::consts::PI * 150.0 * t).sin()
        })
        .collect()
}

#[allow(clippy::cast_precision_loss)]
fn clicks(secs: usize) -> Vec<f32> {
    let mut r = Rng(3);
    let mut v = vec![0.0f32; secs * SR];
    for k in 0..6 * secs {
        let at = (k * 2_711 + 97) % (v.len() - 800);
        for i in 0..800 {
            v[at + i] += r.next() * 0.4 * (-(i as f32) / 60.0).exp();
        }
    }
    v
}

/// Chords with harmonics, a kick drum and hi-hats (a pop-music stand-in).
#[allow(clippy::cast_precision_loss)]
fn music(secs: usize, seed: u64) -> Vec<f32> {
    let mut r = Rng(seed);
    let notes = [220.0f32, 247.0, 262.0, 294.0, 330.0, 349.0, 392.0, 440.0, 494.0, 523.0];
    let beat = SR / 2;
    let mut out = Vec::with_capacity(secs * SR);
    for b in 0..secs * 2 {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let chord: Vec<f32> = (0..3).map(|_| notes[((r.next() + 1.0) * 4.99) as usize]).collect();
        for i in 0..beat {
            let t = i as f32 / SR as f32;
            let env = (-3.0 * t).exp();
            let mut v: f32 = chord
                .iter()
                .map(|f| {
                    let w = 2.0 * std::f32::consts::PI * f * t;
                    0.08 * env * (w.sin() + 0.4 * (2.0 * w).sin() + 0.2 * (3.0 * w).sin())
                })
                .sum();
            if b % 2 == 0 && i < 1200 {
                v += 0.4 * (-(i as f32) / 300.0).exp() * (2.0 * std::f32::consts::PI * 60.0 * t).sin();
            }
            if i < 400 {
                v += r.next() * 0.05 * (-(i as f32) / 80.0).exp();
            }
            out.push(v);
        }
    }
    out
}

fn steady_noise() -> Vec<(&'static str, Pcm)> {
    vec![
        ("silence", pcm(&white(2, 0.0005))),
        ("white", pcm(&white(4, 0.25))),
        ("pink", pcm(&pink(4))),
        ("hum", pcm(&hum(4))),
        ("clicks", pcm(&clicks(4))),
    ]
}

#[test]
fn steady_noise_never_reaches_an_engine() {
    for (name, p) in steady_noise() {
        let a = gate::speech_activity(&p.samples);
        assert!(!a.has_speech(), "{name}: {a:?}");
    }
}

#[test]
fn the_bundled_command_passes_the_activity_gate_even_over_music() {
    let speech = to_pcm16k(BUNDLED).unwrap();
    let a = gate::speech_activity(&speech.samples);
    assert!(a.has_speech(), "clean: {a:?}");
    let m = music(10, 9);
    let mixed: Vec<i16> = speech
        .samples
        .iter()
        .zip(m.iter().cycle())
        .map(|(s, b)| {
            #[allow(clippy::cast_possible_truncation)]
            let v = f32::from(*s) + b * 0.3 * 32767.0 * 0.3;
            v.clamp(-32768.0, 32767.0) as i16
        })
        .collect();
    let a = gate::speech_activity(&mixed);
    assert!(a.has_speech(), "with music: {a:?}");
}

#[tokio::test]
async fn no_noise_clip_is_ever_transcribed_but_commands_are() {
    let Some(dir) = std::env::var_os("ORKESTR_VOICE_TEST_DIR") else {
        eprintln!("ORKESTR_VOICE_TEST_DIR is not set; skipping the whisper noise test");
        return;
    };
    let svc = VoiceService::new(dir.into());
    let s = VoiceEngineSettings {
        whisper_model: std::env::var("ORKESTR_VOICE_TEST_WHISPER").unwrap_or_else(|_| "whisper-base".into()),
        whisper_gpu: std::env::var_os("ORKESTR_VOICE_TEST_GPU").is_some(),
        ..VoiceEngineSettings::default()
    };
    let hints = SttHints { wake_phrases: vec!["Джарвис".into()] };
    let mut clips = steady_noise();
    clips.push(("music1", pcm(&music(5, 11))));
    clips.push(("music2", pcm(&music(5, 12))));
    for (name, p) in &clips {
        let t = svc.transcribe_checked(SttEngine::WhisperCpp, &s, None, p, Some("ru"), &hints).await.unwrap();
        assert!(t.text.is_empty() && t.rejected.is_some(), "{name} produced {t:?}");
    }
    let speech = to_pcm16k(BUNDLED).unwrap();
    let t = svc.transcribe_checked(SttEngine::WhisperCpp, &s, None, &speech, Some("ru"), &hints).await.unwrap();
    let low = t.text.to_lowercase();
    assert!(t.rejected.is_none() && (low.contains("проект") || low.contains("тест")), "{t:?}");
}
