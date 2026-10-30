//! Latency / accuracy / noise-rejection bench of the local recognizers (ADR-0057):
//!
//! `cargo run -p orkestr-voice --example stt_bench -- <voice dir> <dir with *.wav + truth.tsv> <whisper model> <gpu 0|1> <gated 0|1>`
//!
//! `truth.tsv` lines are `file.wav<TAB>expected text`; WAVs without a line are
//! noise and must produce no transcript.
#![allow(clippy::print_stdout, clippy::cast_precision_loss)]

use orkestr_protocol::methods::voice::{SttEngine, VoiceEngineSettings};
use orkestr_voice::{audio, gate, SttHints, VoiceService};

fn wer(reference: &str, hyp: &str) -> f32 {
    let r: Vec<String> = gate::normalize(reference).split(' ').map(str::to_owned).collect();
    let h: Vec<String> = gate::normalize(hyp).split(' ').filter(|w| !w.is_empty()).map(str::to_owned).collect();
    let mut d: Vec<usize> = (0..=h.len()).collect();
    for i in 1..=r.len() {
        let mut prev = d[0];
        d[0] = i;
        for j in 1..=h.len() {
            let cur = (d[j] + 1).min(d[j - 1] + 1).min(prev + usize::from(r[i - 1] != h[j - 1]));
            prev = d[j];
            d[j] = cur;
        }
    }
    d[h.len()] as f32 / r.len().max(1) as f32
}

#[tokio::main]
async fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let [voice, set, model, gpu, gated] = a.as_slice() else {
        panic!("usage: stt_bench <voice dir> <set dir> <whisper model> <gpu 0|1> <gated 0|1>");
    };
    let svc = VoiceService::new(voice.into());
    let s =
        VoiceEngineSettings { whisper_model: model.clone(), whisper_gpu: gpu == "1", ..VoiceEngineSettings::default() };
    let gated = gated == "1";
    let truth: std::collections::HashMap<String, String> =
        std::fs::read_to_string(std::path::Path::new(set).join("truth.tsv"))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| l.split_once('\t').map(|(a, b)| (a.to_owned(), b.to_owned())))
            .collect();
    let t = std::time::Instant::now();
    svc.prewarm(&s).await;
    println!("== {model} gpu={gpu} gated={gated}: model load {} ms", t.elapsed().as_millis());
    let mut files: Vec<_> = std::fs::read_dir(set)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "wav"))
        .collect();
    files.sort();
    let hints = SttHints { wake_phrases: vec!["Джарвис".into()] };
    let (mut times, mut wers, mut noise_through, mut speech_rejected) = (Vec::new(), Vec::new(), 0, 0);
    for f in &files {
        let name = f.file_name().unwrap().to_string_lossy().into_owned();
        let pcm = audio::to_pcm16k(&std::fs::read(f).unwrap()).unwrap();
        let t = std::time::Instant::now();
        let (text, conf, rej) = if gated {
            let r = svc.transcribe_checked(SttEngine::WhisperCpp, &s, None, &pcm, Some("ru"), &hints).await.unwrap();
            (r.text, r.confidence, r.rejected)
        } else {
            (svc.transcribe(SttEngine::WhisperCpp, &s, None, &pcm, Some("ru")).await.unwrap(), None, None)
        };
        let ms = t.elapsed().as_millis();
        match truth.get(&name) {
            Some(exp) => {
                times.push(ms);
                wers.push(wer(exp, &text));
                if text.is_empty() {
                    speech_rejected += 1;
                }
            }
            None if !text.is_empty() => noise_through += 1,
            None => {}
        }
        println!(
            "  {name:38} {ms:6} ms  conf={:<5} rej={:<14} {text:?}",
            conf.map_or("-".into(), |c| format!("{c:.2}")),
            rej.map_or("-".into(), |r| format!("{r:?}"))
        );
    }
    let mean = times.iter().sum::<u128>() as f32 / times.len().max(1) as f32;
    let max = times.iter().max().copied().unwrap_or(0);
    let w = wers.iter().sum::<f32>() / wers.len().max(1) as f32 * 100.0;
    let noise = files.len() - truth.len();
    println!("   speech: mean {mean:.0} ms, max {max} ms, WER {w:.1}%, speech rejected {speech_rejected}/{}; noise with text {noise_through}/{noise}", truth.len());
}
