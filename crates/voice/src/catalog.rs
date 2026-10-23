//! The voice package catalog: engine runtimes, STT models and TTS voices
//! with pinned URLs, sizes and SHA-256 checksums (ADR-0046). Hashes were
//! taken from the publishers' metadata (GitHub release digests, Hugging Face
//! LFS ids) or computed from the published file on 2026-10-23.

use orkestr_protocol::methods::voice::{SttEngine, TtsEngine, VoicePackageKind};

/// How a downloaded file is installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unpack {
    /// Stored as-is under this file name.
    Raw(&'static str),
    /// A zip archive (a single top-level directory is stripped).
    Zip,
    /// A `.tar.bz2` archive (unpacked with the system `tar`; a single top-level directory is stripped).
    TarBz2,
}

/// One downloaded file.
#[derive(Debug, Clone, Copy)]
pub struct PackageFile {
    /// HTTPS URL.
    pub url: &'static str,
    /// Size in bytes.
    pub size: u64,
    /// Lower-case hex SHA-256.
    pub sha256: &'static str,
    /// Install mode.
    pub unpack: Unpack,
}

/// A catalog entry.
#[derive(Debug, Clone, Copy)]
pub struct Package {
    /// Id (directory name under `voice/models`).
    pub id: &'static str,
    /// Title.
    pub title: &'static str,
    /// Kind.
    pub kind: VoicePackageKind,
    /// STT engine.
    pub stt: Option<SttEngine>,
    /// TTS engine.
    pub tts: Option<TtsEngine>,
    /// Languages (`*` = multilingual).
    pub languages: &'static [&'static str],
    /// Files.
    pub files: &'static [PackageFile],
    /// Installed first.
    pub depends: &'static [&'static str],
    /// Suggested for Russian out of the box.
    pub recommended: bool,
    /// Only for Windows x86-64 (native binaries).
    pub windows_x64_only: bool,
    /// License.
    pub license: &'static str,
}

impl Package {
    /// Total download size.
    #[must_use]
    pub fn download_bytes(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }

    /// Whether this OS/CPU can use the package.
    #[must_use]
    pub fn supported(&self) -> bool {
        !self.windows_x64_only || (cfg!(windows) && cfg!(target_arch = "x86_64"))
    }

    /// Download host.
    #[must_use]
    pub fn source(&self) -> String {
        self.files
            .first()
            .and_then(|f| f.url.strip_prefix("https://"))
            .and_then(|r| r.split('/').next())
            .unwrap_or_default()
            .to_owned()
    }
}

const fn f(url: &'static str, size: u64, sha256: &'static str, unpack: Unpack) -> PackageFile {
    PackageFile { url, size, sha256, unpack }
}

const STT_W: Option<SttEngine> = Some(SttEngine::WhisperCpp);
const STT_V: Option<SttEngine> = Some(SttEngine::Vosk);
const STT_S: Option<SttEngine> = Some(SttEngine::SherpaOnnx);
const TTS_P: Option<TtsEngine> = Some(TtsEngine::Piper);
const RU: &[&str] = &["ru"];
const MULTI: &[&str] = &["*"];

macro_rules! piper_voice {
    ($id:literal, $title:literal, $name:literal, $onnx_sha:literal, $json_size:literal, $json_sha:literal, $rec:expr) => {
        Package {
            id: $id,
            title: $title,
            kind: VoicePackageKind::TtsVoice,
            stt: None,
            tts: TTS_P,
            languages: RU,
            files: &[
                f(
                    concat!(
                        "https://huggingface.co/rhasspy/piper-voices/resolve/v1.0.0/ru/ru_RU/",
                        $name,
                        "/medium/ru_RU-",
                        $name,
                        "-medium.onnx"
                    ),
                    63_201_294,
                    $onnx_sha,
                    Unpack::Raw(concat!("ru_RU-", $name, "-medium.onnx")),
                ),
                f(
                    concat!(
                        "https://huggingface.co/rhasspy/piper-voices/resolve/v1.0.0/ru/ru_RU/",
                        $name,
                        "/medium/ru_RU-",
                        $name,
                        "-medium.onnx.json"
                    ),
                    $json_size,
                    $json_sha,
                    Unpack::Raw(concat!("ru_RU-", $name, "-medium.onnx.json")),
                ),
            ],
            depends: &["piper-runtime"],
            recommended: $rec,
            windows_x64_only: false,
            license: "see MODEL_CARD (rhasspy/piper-voices)",
        }
    };
}

/// The catalog.
pub static CATALOG: &[Package] = &[
    // --- whisper.cpp ------------------------------------------------------
    Package {
        id: "whisper-runtime",
        title: "whisper.cpp CLI (CPU, b5454)",
        kind: VoicePackageKind::Runtime,
        stt: STT_W,
        tts: None,
        languages: MULTI,
        files: &[f(
            "https://github.com/ggml-org/whisper.cpp/releases/download/b5454/whisper-bin-x64.zip",
            8_928_640,
            "6ba69e3482d7826214f90a6a9c84ca07782aec1e1d0c6a7c30c994fd5d816ccb",
            Unpack::Zip,
        )],
        depends: &[],
        recommended: true,
        windows_x64_only: true,
        license: "MIT",
    },
    Package {
        id: "whisper-runtime-cuda",
        title: "whisper.cpp CLI (NVIDIA CUDA 11.8, b5454)",
        kind: VoicePackageKind::Runtime,
        stt: STT_W,
        tts: None,
        languages: MULTI,
        files: &[f(
            "https://github.com/ggml-org/whisper.cpp/releases/download/b5454/whisper-bin-win-cuda-11.8.0-x64.zip",
            284_761_231,
            "e81c86c542be6e3d7b7245392d4e1c52ab6e5c61dbe38721754bc0ab59d4d027",
            Unpack::Zip,
        )],
        depends: &[],
        recommended: false,
        windows_x64_only: true,
        license: "MIT (CUDA runtime: NVIDIA EULA)",
    },
    Package {
        id: "whisper-base",
        title: "Whisper base (multilingual, 148 MB)",
        kind: VoicePackageKind::SttModel,
        stt: STT_W,
        tts: None,
        languages: MULTI,
        files: &[f(
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin",
            147_951_465,
            "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe",
            Unpack::Raw("ggml-base.bin"),
        )],
        depends: &["whisper-runtime"],
        recommended: true,
        windows_x64_only: false,
        license: "MIT",
    },
    Package {
        id: "whisper-small",
        title: "Whisper small (multilingual, 488 MB)",
        kind: VoicePackageKind::SttModel,
        stt: STT_W,
        tts: None,
        languages: MULTI,
        files: &[f(
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin",
            487_601_967,
            "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
            Unpack::Raw("ggml-small.bin"),
        )],
        depends: &["whisper-runtime"],
        recommended: false,
        windows_x64_only: false,
        license: "MIT",
    },
    Package {
        id: "whisper-large-v3-turbo-q5",
        title: "Whisper large-v3-turbo q5_0 (multilingual, 574 MB; best with GPU)",
        kind: VoicePackageKind::SttModel,
        stt: STT_W,
        tts: None,
        languages: MULTI,
        files: &[f(
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin",
            574_041_195,
            "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2",
            Unpack::Raw("ggml-large-v3-turbo-q5_0.bin"),
        )],
        depends: &["whisper-runtime"],
        recommended: false,
        windows_x64_only: false,
        license: "MIT",
    },
    // --- Vosk -------------------------------------------------------------
    Package {
        id: "vosk-runtime",
        title: "Vosk runtime (libvosk 0.3.45)",
        kind: VoicePackageKind::Runtime,
        stt: STT_V,
        tts: None,
        languages: MULTI,
        files: &[f(
            "https://github.com/alphacep/vosk-api/releases/download/v0.3.45/vosk-win64-0.3.45.zip",
            14_882_445,
            "f1dcc9cca460630f81ea8f71794f69c80bed6556d2a4e6237b5785e1d2dff34b",
            Unpack::Zip,
        )],
        depends: &[],
        recommended: true,
        windows_x64_only: true,
        license: "Apache-2.0",
    },
    Package {
        id: "vosk-small-ru",
        title: "Vosk small Russian 0.22 (46 MB)",
        kind: VoicePackageKind::SttModel,
        stt: STT_V,
        tts: None,
        languages: RU,
        files: &[f(
            "https://alphacephei.com/vosk/models/vosk-model-small-ru-0.22.zip",
            46_236_750,
            "961d5ff98a17f4aa6de69864d0aa71fa5bac682301d2b5d17a3f24c5c99a46d4",
            Unpack::Zip,
        )],
        depends: &["vosk-runtime"],
        recommended: true,
        windows_x64_only: false,
        license: "Apache-2.0",
    },
    Package {
        id: "vosk-ru",
        title: "Vosk Russian 0.42 (full, 1.9 GB)",
        kind: VoicePackageKind::SttModel,
        stt: STT_V,
        tts: None,
        languages: RU,
        files: &[f(
            "https://alphacephei.com/vosk/models/vosk-model-ru-0.42.zip",
            1_937_602_113,
            "00eb9e5ed8230492da1acf702cd3548e1282256a5fdde8f188253e77217e9429",
            Unpack::Zip,
        )],
        depends: &["vosk-runtime"],
        recommended: false,
        windows_x64_only: false,
        license: "Apache-2.0",
    },
    // --- sherpa-onnx ------------------------------------------------------
    Package {
        id: "sherpa-runtime",
        title: "sherpa-onnx 1.13.8 CLI",
        kind: VoicePackageKind::Runtime,
        stt: STT_S,
        tts: None,
        languages: MULTI,
        files: &[f(
            "https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.8/sherpa-onnx-v1.13.8-win-x64-shared-MD-Release-no-tts.tar.bz2",
            19_164_933,
            "876e6b89b8cf84a3a1b375a397507f2cfe9c227c2a945411a11a668475fcb5d3",
            Unpack::TarBz2,
        )],
        depends: &[],
        recommended: false,
        windows_x64_only: true,
        license: "Apache-2.0",
    },
    Package {
        id: "sherpa-zipformer-small-ru-int8",
        title: "Streaming zipformer small Russian (Vosk, int8, 24 MB)",
        kind: VoicePackageKind::SttModel,
        stt: STT_S,
        tts: None,
        languages: RU,
        files: &[f(
            "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-streaming-zipformer-small-ru-vosk-int8-2025-08-16.tar.bz2",
            24_110_855,
            "6ba68a01ff3c5445aaf2d61e9b97b026f1149dcc9049d11af3f44f55176341d8",
            Unpack::TarBz2,
        )],
        depends: &["sherpa-runtime"],
        recommended: false,
        windows_x64_only: false,
        license: "Apache-2.0",
    },
    Package {
        id: "sherpa-zipformer-small-ru",
        title: "Streaming zipformer small Russian (Vosk, fp32, 90 MB)",
        kind: VoicePackageKind::SttModel,
        stt: STT_S,
        tts: None,
        languages: RU,
        files: &[f(
            "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-streaming-zipformer-small-ru-vosk-2025-08-16.tar.bz2",
            89_653_224,
            "cc2e99ed0c67cae8801170e7b7539b4cac00b716076af86f974bf5b888d9370c",
            Unpack::TarBz2,
        )],
        depends: &["sherpa-runtime"],
        recommended: false,
        windows_x64_only: false,
        license: "Apache-2.0",
    },
    // --- Piper ------------------------------------------------------------
    Package {
        id: "piper-runtime",
        title: "Piper 2023.11.14-2",
        kind: VoicePackageKind::Runtime,
        stt: None,
        tts: TTS_P,
        languages: MULTI,
        files: &[f(
            "https://github.com/rhasspy/piper/releases/download/2023.11.14-2/piper_windows_amd64.zip",
            22_477_236,
            "f3c58906402b24f3a96d92145f58acba6d86c9b5db896d207f78dc80811efcea",
            Unpack::Zip,
        )],
        depends: &[],
        recommended: true,
        windows_x64_only: true,
        license: "MIT (espeak-ng: GPL-3.0)",
    },
    piper_voice!(
        "piper-ru-irina",
        "Piper ru_RU Irina (medium)",
        "irina",
        "8ff38212d23da300bbe3705c645e6e5b9475f0bfde01558eb17813e22acaaaaa",
        4765,
        "c2ec28bb38e2b59e93b959b3e40348c1afebbd272f30fed5d41205d08e98a9d7",
        true
    ),
    piper_voice!(
        "piper-ru-denis",
        "Piper ru_RU Denis (medium)",
        "denis",
        "15fab56e11a097858ee115545d0f697fc2a316c41a291a5362349fb870411b0a",
        4823,
        "831c860dac0b5073eaa81610a0a638ec23d90a6cf8e5f871b4485c2cec3767c8",
        true
    ),
    piper_voice!(
        "piper-ru-dmitri",
        "Piper ru_RU Dmitri (medium)",
        "dmitri",
        "f073356ebc4bd0f80c5af58df2953a5988bd5bdab1eb38635ce960b071fbefcb",
        4824,
        "667ef3117bc642c2892dff7690d8bdc8ca4228aeaa783b2dc1416df632855e0d",
        true
    ),
    piper_voice!(
        "piper-ru-ruslan",
        "Piper ru_RU Ruslan (medium)",
        "ruslan",
        "72a5f88e0b20928064eb45d88e1daa21f8af62d18613580d32cbb4aed48dcf7f",
        4882,
        "706a4fb17bc304abd07809b552deae615e64dcbffbfbd09854ba37ca59e88117",
        true
    ),
];

/// Looks a package up.
#[must_use]
pub fn package(id: &str) -> Option<&'static Package> {
    CATALOG.iter().find(|p| p.id == id)
}

/// Packages of an STT engine.
#[must_use]
pub fn stt_packages(e: SttEngine) -> Vec<&'static Package> {
    CATALOG.iter().filter(|p| p.stt == Some(e)).collect()
}

/// Packages of a TTS engine.
#[must_use]
pub fn tts_packages(e: TtsEngine) -> Vec<&'static Package> {
    CATALOG.iter().filter(|p| p.tts == Some(e)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_is_consistent() {
        let mut ids: Vec<_> = CATALOG.iter().map(|p| p.id).collect();
        ids.sort_unstable();
        let n = ids.len();
        ids.dedup();
        assert_eq!(n, ids.len(), "unique ids");
        for p in CATALOG {
            assert!(p.id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'), "{}", p.id);
            for d in p.depends {
                assert!(package(d).is_some(), "{} depends on unknown {d}", p.id);
            }
            for file in p.files {
                assert!(file.url.starts_with("https://"), "{}", file.url);
                assert_eq!(file.sha256.len(), 64, "{}", p.id);
                assert!(file.sha256.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
                assert!(file.size > 0);
            }
            assert!(p.stt.is_some() != p.tts.is_some(), "{} belongs to exactly one engine", p.id);
        }
    }
}
