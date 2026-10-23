//! WAV decoding / encoding and resampling. Every engine receives 16 kHz mono
//! 16-bit PCM ([`SAMPLE_RATE`]); synthesized audio is returned as encoded
//! bytes (WAV or MP3).

use crate::VoiceError;

/// Sample rate fed to every STT engine.
pub const SAMPLE_RATE: u32 = 16_000;

/// 16 kHz mono PCM.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Pcm {
    /// Samples.
    pub samples: Vec<i16>,
}

impl Pcm {
    /// Duration in seconds.
    #[must_use]
    pub fn seconds(&self) -> f32 {
        #[allow(clippy::cast_precision_loss)]
        let n = self.samples.len() as f32;
        n / SAMPLE_RATE as f32
    }

    /// Raw little-endian bytes.
    #[must_use]
    pub fn to_le_bytes(&self) -> Vec<u8> {
        self.samples.iter().flat_map(|s| s.to_le_bytes()).collect()
    }

    /// Encoded as a 16 kHz mono WAV file.
    #[must_use]
    pub fn to_wav(&self) -> Vec<u8> {
        encode_wav(&self.samples, SAMPLE_RATE)
    }
}

/// Encodes 16-bit mono PCM as WAV.
#[must_use]
pub fn encode_wav(samples: &[i16], rate: u32) -> Vec<u8> {
    let data_len = u32::try_from(samples.len() * 2).unwrap_or(u32::MAX);
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// A decoded WAV file (mono, original rate).
#[derive(Debug, Clone, PartialEq)]
pub struct Decoded {
    /// Mono samples in -1..=1.
    pub samples: Vec<f32>,
    /// Sample rate.
    pub rate: u32,
}

fn u16_at(b: &[u8], i: usize) -> Option<u16> {
    b.get(i..i + 2).map(|s| u16::from_le_bytes([s[0], s[1]]))
}
fn u32_at(b: &[u8], i: usize) -> Option<u32> {
    b.get(i..i + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// Decodes a PCM16 / PCM24 / PCM32 / float32 WAV file, mixing channels down to mono.
pub fn decode_wav(bytes: &[u8]) -> Result<Decoded, VoiceError> {
    let bad = |m: &str| VoiceError::BadRequest(format!("not a supported WAV file: {m}"));
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(bad("missing RIFF/WAVE header"));
    }
    let mut i = 12;
    let mut fmt: Option<(u16, u16, u32, u16)> = None;
    let mut data: Option<&[u8]> = None;
    while i + 8 <= bytes.len() {
        let id = &bytes[i..i + 4];
        let len = u32_at(bytes, i + 4).unwrap_or(0) as usize;
        let body_start = i + 8;
        let body_end = body_start.saturating_add(len).min(bytes.len());
        let body = &bytes[body_start..body_end];
        if id == b"fmt " {
            let format = u16_at(body, 0).ok_or_else(|| bad("short fmt"))?;
            let channels = u16_at(body, 2).ok_or_else(|| bad("short fmt"))?;
            let rate = u32_at(body, 4).ok_or_else(|| bad("short fmt"))?;
            let bits = u16_at(body, 14).ok_or_else(|| bad("short fmt"))?;
            // WAVE_FORMAT_EXTENSIBLE: the sub-format GUID starts with the real format.
            let format = if format == 0xFFFE { u16_at(body, 24).unwrap_or(1) } else { format };
            fmt = Some((format, channels, rate, bits));
        } else if id == b"data" {
            data = Some(body);
        }
        i = body_start + len + (len & 1);
    }
    let (format, channels, rate, bits) = fmt.ok_or_else(|| bad("no fmt chunk"))?;
    let data = data.ok_or_else(|| bad("no data chunk"))?;
    if channels == 0 || rate == 0 {
        return Err(bad("zero channels or rate"));
    }
    let width = usize::from(bits / 8);
    let frame = width * usize::from(channels);
    if frame == 0 {
        return Err(bad("zero sample width"));
    }
    let sample = |s: &[u8]| -> Option<f32> {
        Some(match (format, bits) {
            (1, 16) => f32::from(i16::from_le_bytes([s[0], s[1]])) / 32768.0,
            (1, 24) => {
                #[allow(clippy::cast_precision_loss)]
                let v = (i32::from_le_bytes([0, s[0], s[1], s[2]]) >> 8) as f32;
                v / 8_388_608.0
            }
            (1, 32) => {
                #[allow(clippy::cast_precision_loss)]
                let v = i32::from_le_bytes([s[0], s[1], s[2], s[3]]) as f32;
                v / 2_147_483_648.0
            }
            (3, 32) => f32::from_le_bytes([s[0], s[1], s[2], s[3]]),
            _ => return None,
        })
    };
    let mut samples = Vec::with_capacity(data.len() / frame);
    for f in data.chunks_exact(frame) {
        let mut sum = 0.0f32;
        for c in f.chunks_exact(width) {
            sum += sample(c).ok_or_else(|| bad(&format!("format {format} with {bits} bits")))?;
        }
        samples.push(sum / f32::from(channels));
    }
    Ok(Decoded { samples, rate })
}

/// Linear resampling to `to` Hz.
#[must_use]
pub fn resample(samples: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || samples.is_empty() {
        return samples.to_vec();
    }
    let ratio = f64::from(from) / f64::from(to);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
    let n = ((samples.len() as f64) / ratio).floor() as usize;
    (0..n)
        .map(|i| {
            #[allow(clippy::cast_precision_loss)]
            let pos = i as f64 * ratio;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let i0 = pos.floor() as usize;
            let i1 = (i0 + 1).min(samples.len() - 1);
            #[allow(clippy::cast_possible_truncation)]
            let t = (pos - pos.floor()) as f32;
            samples[i0] * (1.0 - t) + samples[i1] * t
        })
        .collect()
}

/// Decodes any supported WAV into 16 kHz mono PCM.
pub fn to_pcm16k(bytes: &[u8]) -> Result<Pcm, VoiceError> {
    let d = decode_wav(bytes)?;
    let s = resample(&d.samples, d.rate, SAMPLE_RATE);
    #[allow(clippy::cast_possible_truncation)]
    let samples = s.iter().map(|v| (v.clamp(-1.0, 1.0) * 32767.0) as i16).collect();
    Ok(Pcm { samples })
}

/// Root-mean-square level of a WAV file (0..=1), for "non-silent" checks.
pub fn wav_rms(bytes: &[u8]) -> Result<f32, VoiceError> {
    let d = decode_wav(bytes)?;
    if d.samples.is_empty() {
        return Ok(0.0);
    }
    #[allow(clippy::cast_precision_loss)]
    let n = d.samples.len() as f32;
    Ok((d.samples.iter().map(|v| v * v).sum::<f32>() / n).sqrt())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_roundtrip_and_resample() {
        let tone: Vec<i16> = (0..22_050)
            .map(|i| {
                #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
                let v = ((i as f32 * 440.0 * std::f32::consts::TAU / 22_050.0).sin() * 10_000.0) as i16;
                v
            })
            .collect();
        let wav = encode_wav(&tone, 22_050);
        let d = decode_wav(&wav).unwrap();
        assert_eq!(d.rate, 22_050);
        assert_eq!(d.samples.len(), 22_050);
        let pcm = to_pcm16k(&wav).unwrap();
        assert!((15_990..=16_000).contains(&pcm.samples.len()), "{}", pcm.samples.len());
        assert!(wav_rms(&wav).unwrap() > 0.1);
        assert_eq!(decode_wav(&pcm.to_wav()).unwrap().rate, 16_000);
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode_wav(b"RIFFxxxx").is_err());
        assert!(decode_wav(b"").is_err());
    }
}
