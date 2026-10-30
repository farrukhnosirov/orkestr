//! Barge-in end-to-end (ADR-0053): a synthesized Russian «Стоп!» (Piper,
//! `data/ru_stop.wav`, 16 kHz) streamed in 100 ms chunks — like the voice
//! worklet sends them — through the Vosk grammar recognizer. Measures the
//! detection latency from the end of the word. Needs
//! `ORKESTR_VOICE_TEST_DIR=<ORKESTR_HOME>/voice` with `vosk-runtime` +
//! `vosk-small-ru` installed; without it the test prints a note and passes.

use std::time::Instant;

use orkestr_protocol::methods::voice::{BargeEngine, BargeHit, BargeInConfig, StopKind, VoiceEngineSettings};
use orkestr_voice::audio::{to_pcm16k, SAMPLE_RATE};
use orkestr_voice::VoiceService;

const STOP: &[u8] = include_bytes!("data/ru_stop.wav");
const COMMAND: &[u8] = include_bytes!("data/ru_command.wav");
const CHUNK: usize = (SAMPLE_RATE / 10) as usize; // 100 ms
const LEAD_MS: usize = 500;
const TAIL_MS: usize = 1500;

fn service() -> Option<VoiceService> {
    let dir = std::env::var_os("ORKESTR_VOICE_TEST_DIR")?;
    let svc = VoiceService::new(dir.into());
    if svc.models().is_installed("vosk-runtime") && svc.models().is_installed("vosk-small-ru") {
        Some(svc)
    } else {
        eprintln!("vosk-small-ru is not installed; skipping");
        None
    }
}

/// Low room noise (deterministic), so the stream is not digital silence.
fn noise(ms: usize, seed: &mut u32) -> Vec<i16> {
    (0..ms * SAMPLE_RATE as usize / 1000)
        .map(|_| {
            *seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            #[allow(clippy::cast_possible_wrap)]
            let v = ((*seed >> 16) as i16) / 400;
            v
        })
        .collect()
}

struct Run {
    hit: Option<BargeHit>,
    /// Audio position (ms) at the end of the chunk that produced the hit.
    at_ms: usize,
    /// Wall time spent in that chunk's feed call.
    feed_ms: u128,
    max_feed_ms: u128,
}

async fn stream(svc: &VoiceService, speech: &[i16], spoken: Option<&str>) -> Run {
    let (id, engine) =
        svc.barge_open(&BargeInConfig::default(), &VoiceEngineSettings::default(), Some("ru")).await.unwrap().unwrap();
    assert_eq!(engine, BargeEngine::VoskGrammar);
    let mut seed = 7;
    let mut all = noise(LEAD_MS, &mut seed);
    all.extend_from_slice(speech);
    all.extend(noise(TAIL_MS, &mut seed));
    let mut run = Run { hit: None, at_ms: 0, feed_ms: 0, max_feed_ms: 0 };
    for (i, chunk) in all.chunks(CHUNK).enumerate() {
        let t = Instant::now();
        let hit = svc.barge_feed(id, chunk.to_vec(), spoken).await.unwrap();
        let took = t.elapsed().as_millis();
        run.max_feed_ms = run.max_feed_ms.max(took);
        if hit.is_some() {
            run.hit = hit;
            run.at_ms = (i + 1) * 100;
            run.feed_ms = took;
            break;
        }
    }
    svc.barge_close(id);
    run
}

#[tokio::test]
async fn russian_stop_is_spotted_quickly_by_the_vosk_grammar() {
    let Some(svc) = service() else { return };
    let stop = to_pcm16k(STOP).unwrap().samples;
    let word_ms = stop.len() * 1000 / SAMPLE_RATE as usize;
    // Warm-up: the first session loads the model (hundreds of ms).
    let t = Instant::now();
    let warm = stream(&svc, &stop, None).await;
    eprintln!("first session incl. model load: {} ms", t.elapsed().as_millis());
    assert!(warm.hit.is_some());

    let run = stream(&svc, &stop, None).await;
    let hit = run.hit.expect("«Стоп!» must be spotted");
    assert_eq!(hit.kind, StopKind::Hard, "{hit:?}");
    assert_eq!(hit.phrase, "стоп");
    let speech_end = LEAD_MS + word_ms;
    // Latency = audio that had to arrive after the word ended + the feed call itself.
    let latency = run.at_ms.saturating_sub(speech_end) as u128 + run.feed_ms;
    eprintln!(
        "barge-in: word {word_ms} ms, hit at {} ms (word ended at {speech_end} ms), feed {} ms (max {} ms) → latency {latency} ms; heard «{}» conf {}",
        run.at_ms, run.feed_ms, run.max_feed_ms, hit.heard, hit.confidence
    );
    assert!(latency < 400, "latency {latency} ms");
    assert!(run.max_feed_ms < 100, "a 100 ms chunk must decode faster than real time: {} ms", run.max_feed_ms);
}

#[tokio::test]
async fn a_command_is_not_a_stop_and_self_echo_is_ignored() {
    let Some(svc) = service() else { return };
    let command = to_pcm16k(COMMAND).unwrap().samples;
    let run = stream(&svc, &command, None).await;
    assert!(run.hit.is_none(), "«Оркестр, открой проект и запусти тесты» is not a stop phrase: {:?}", run.hit);
    // The assistant itself is saying «стоп»: its echo must not interrupt it.
    let stop = to_pcm16k(STOP).unwrap().samples;
    let echo = stream(&svc, &stop, Some("Чтобы остановить, скажите стоп, сэр.")).await;
    assert!(echo.hit.is_none(), "self-echo: {:?}", echo.hit);
}
