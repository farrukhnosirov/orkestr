//! Vosk speech recognition through `libvosk` loaded at run time (§33, ADR-0046).
//!
//! The Vosk runtime (`libvosk.dll` plus its MinGW runtime DLLs on Windows,
//! `libvosk.so` / `libvosk.dylib` elsewhere) is downloaded by the voice model
//! manager; nothing is linked at build time. This is FFI, so it lives in the
//! platform crate (engineering contract §4). The `unsafe` blocks below are
//! limited to:
//!
//! - loading the library and resolving the C symbols of the stable Vosk C
//!   API (`vosk_api.h`, v0.3.x; the grammar / streaming symbols are optional);
//! - calling them with pointers that this module owns: model and recognizer
//!   handles are created and freed here only, PCM slices are borrowed for
//!   the duration of one call, and returned C strings are copied before the
//!   next call into the same recognizer.
//!
//! The library is never unloaded (it is kept in an `Arc` by every model), so
//! resolved function pointers stay valid for the life of the process.
#![allow(unsafe_code)]

use std::ffi::{c_char, c_float, c_int, c_short, c_void, CStr, CString};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::{PlatformError, PlatformResult};

type ModelNew = unsafe extern "C" fn(*const c_char) -> *mut c_void;
type ModelFree = unsafe extern "C" fn(*mut c_void);
type RecognizerNew = unsafe extern "C" fn(*mut c_void, c_float) -> *mut c_void;
type RecognizerFree = unsafe extern "C" fn(*mut c_void);
type AcceptWaveformS = unsafe extern "C" fn(*mut c_void, *const c_short, c_int) -> c_int;
type ResultFn = unsafe extern "C" fn(*mut c_void) -> *const c_char;
type SetLogLevel = unsafe extern "C" fn(c_int);
type RecognizerNewGrm = unsafe extern "C" fn(*mut c_void, c_float, *const c_char) -> *mut c_void;
type RecognizerSetInt = unsafe extern "C" fn(*mut c_void, c_int);
type RecognizerReset = unsafe extern "C" fn(*mut c_void);

struct Api {
    _lib: libloading::Library,
    model_new: ModelNew,
    model_free: ModelFree,
    recognizer_new: RecognizerNew,
    recognizer_free: RecognizerFree,
    accept_waveform_s: AcceptWaveformS,
    result: ResultFn,
    final_result: ResultFn,
    // Grammar / streaming recognition (barge-in, ADR-0053). Optional so an
    // older libvosk still serves plain transcription.
    recognizer_new_grm: Option<RecognizerNewGrm>,
    partial_result: Option<ResultFn>,
    set_words: Option<RecognizerSetInt>,
    reset: Option<RecognizerReset>,
}

/// A loaded `libvosk`.
#[derive(Clone)]
pub struct VoskLibrary {
    api: Arc<Api>,
}

impl std::fmt::Debug for VoskLibrary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VoskLibrary").finish_non_exhaustive()
    }
}

/// File name of the Vosk library on this OS.
#[must_use]
pub fn library_file_name() -> &'static str {
    if cfg!(windows) {
        "libvosk.dll"
    } else if cfg!(target_os = "macos") {
        "libvosk.dylib"
    } else {
        "libvosk.so"
    }
}

fn load_error(e: impl std::fmt::Display) -> PlatformError {
    PlatformError::Native(format!("could not load the Vosk runtime: {e}"))
}

#[cfg(windows)]
fn open(path: &Path) -> Result<libloading::Library, libloading::Error> {
    // LOAD_WITH_ALTERED_SEARCH_PATH: resolve the MinGW runtime DLLs that ship
    // next to libvosk.dll from that directory, not from the process's.
    // SAFETY: loading a library runs its initializers; the file comes from
    // the checksum-verified Vosk runtime package.
    unsafe {
        libloading::os::windows::Library::load_with_flags(path, libloading::os::windows::LOAD_WITH_ALTERED_SEARCH_PATH)
            .map(Into::into)
    }
}

#[cfg(not(windows))]
fn open(path: &Path) -> Result<libloading::Library, libloading::Error> {
    // SAFETY: see the Windows variant.
    unsafe { libloading::Library::new(path) }
}

impl VoskLibrary {
    /// Loads `libvosk` from `dir` (the runtime package directory) and
    /// silences its logging.
    pub fn load(dir: &Path) -> PlatformResult<Self> {
        let path = dir.join(library_file_name());
        if !path.is_file() {
            return Err(PlatformError::Native(format!("{} is missing", path.display())));
        }
        let lib = open(&path).map_err(load_error)?;
        // SAFETY: the symbol types match vosk_api.h (v0.3.x).
        let api = unsafe {
            let model_new = *lib.get::<ModelNew>(b"vosk_model_new\0").map_err(load_error)?;
            let model_free = *lib.get::<ModelFree>(b"vosk_model_free\0").map_err(load_error)?;
            let recognizer_new = *lib.get::<RecognizerNew>(b"vosk_recognizer_new\0").map_err(load_error)?;
            let recognizer_free = *lib.get::<RecognizerFree>(b"vosk_recognizer_free\0").map_err(load_error)?;
            let accept_waveform_s =
                *lib.get::<AcceptWaveformS>(b"vosk_recognizer_accept_waveform_s\0").map_err(load_error)?;
            let result = *lib.get::<ResultFn>(b"vosk_recognizer_result\0").map_err(load_error)?;
            let final_result = *lib.get::<ResultFn>(b"vosk_recognizer_final_result\0").map_err(load_error)?;
            if let Ok(set_log) = lib.get::<SetLogLevel>(b"vosk_set_log_level\0") {
                set_log(-1);
            }
            let recognizer_new_grm = lib.get::<RecognizerNewGrm>(b"vosk_recognizer_new_grm\0").ok().map(|f| *f);
            let partial_result = lib.get::<ResultFn>(b"vosk_recognizer_partial_result\0").ok().map(|f| *f);
            let set_words = lib.get::<RecognizerSetInt>(b"vosk_recognizer_set_words\0").ok().map(|f| *f);
            let reset = lib.get::<RecognizerReset>(b"vosk_recognizer_reset\0").ok().map(|f| *f);
            Api {
                model_new,
                model_free,
                recognizer_new,
                recognizer_free,
                accept_waveform_s,
                result,
                final_result,
                recognizer_new_grm,
                partial_result,
                set_words,
                reset,
                _lib: lib,
            }
        };
        Ok(Self { api: Arc::new(api) })
    }

    /// Loads a model directory (slow: hundreds of ms to seconds).
    pub fn model(&self, dir: &Path) -> PlatformResult<VoskModel> {
        let s = dir.to_str().ok_or_else(|| PlatformError::Native("model path is not valid UTF-8".into()))?;
        if cfg!(windows) && !s.is_ascii() {
            // libvosk opens files with narrow (ANSI) paths on Windows.
            return Err(PlatformError::Native(format!(
                "the Vosk model path contains non-ASCII characters, which libvosk cannot open on Windows: {s}"
            )));
        }
        let c = CString::new(s).map_err(|_| PlatformError::Native("model path contains NUL".into()))?;
        // SAFETY: `c` is a valid NUL-terminated string for the call.
        let ptr = unsafe { (self.api.model_new)(c.as_ptr()) };
        if ptr.is_null() {
            return Err(PlatformError::Native(format!("Vosk could not load the model at {s}")));
        }
        Ok(VoskModel { api: Arc::clone(&self.api), ptr, path: dir.to_path_buf() })
    }
}

/// A loaded Vosk model. Vosk models are immutable after loading and safe to
/// share between threads (each recognition creates its own recognizer).
pub struct VoskModel {
    api: Arc<Api>,
    ptr: *mut c_void,
    path: PathBuf,
}

// SAFETY: a VoskModel is read-only after `vosk_model_new` (the Vosk docs
// state that one model may be shared by recognizers on several threads);
// the handle is freed only in Drop.
unsafe impl Send for VoskModel {}
// SAFETY: see above.
unsafe impl Sync for VoskModel {}

impl std::fmt::Debug for VoskModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VoskModel").field("path", &self.path).finish_non_exhaustive()
    }
}

impl Drop for VoskModel {
    fn drop(&mut self) {
        // SAFETY: `ptr` came from vosk_model_new and is freed exactly once.
        unsafe { (self.api.model_free)(self.ptr) }
    }
}

fn read_text(ptr: *const c_char) -> String {
    if ptr.is_null() {
        return String::new();
    }
    // SAFETY: Vosk returns a NUL-terminated JSON string owned by the
    // recognizer, valid until the next call on it; we copy it right away.
    let json = unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned();
    serde_json::from_str::<serde_json::Value>(&json)
        .ok()
        .and_then(|v| v.get("text").and_then(|t| t.as_str()).map(str::to_owned))
        .unwrap_or_default()
}

impl VoskModel {
    /// Directory the model was loaded from.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Recognizes a whole utterance of 16-bit mono PCM at `sample_rate`.
    pub fn recognize(&self, pcm: &[i16], sample_rate: u32) -> PlatformResult<String> {
        #[allow(clippy::cast_precision_loss)]
        // SAFETY: the model handle is valid for the life of `self`.
        let rec = unsafe { (self.api.recognizer_new)(self.ptr, sample_rate as c_float) };
        if rec.is_null() {
            return Err(PlatformError::Native("Vosk could not create a recognizer".into()));
        }
        let mut parts: Vec<String> = Vec::new();
        for chunk in pcm.chunks(8000) {
            let len = c_int::try_from(chunk.len()).unwrap_or(0);
            // SAFETY: `chunk` is a valid slice of `len` i16 samples for the call.
            let done = unsafe { (self.api.accept_waveform_s)(rec, chunk.as_ptr(), len) };
            if done == 1 {
                // SAFETY: `rec` is valid; the string is copied by read_text.
                parts.push(read_text(unsafe { (self.api.result)(rec) }));
            }
        }
        // SAFETY: as above.
        parts.push(read_text(unsafe { (self.api.final_result)(rec) }));
        // SAFETY: `rec` came from vosk_recognizer_new and is freed exactly once.
        unsafe { (self.api.recognizer_free)(rec) };
        Ok(parts.into_iter().filter(|p| !p.trim().is_empty()).collect::<Vec<_>>().join(" "))
    }
}

/// One recognized word of a final result.
#[derive(Debug, Clone, PartialEq)]
pub struct VoskWord {
    /// The word.
    pub word: String,
    /// Confidence 0..=1.
    pub conf: f32,
}

/// What a [`VoskStream`] produced for a chunk.
#[derive(Debug, Clone, PartialEq)]
pub enum VoskEvent {
    /// Nothing new.
    None,
    /// The utterance so far (no confidence yet).
    Partial(String),
    /// An utterance ended (endpoint): its text and words with confidences.
    Final {
        /// The text.
        text: String,
        /// Words with confidences.
        words: Vec<VoskWord>,
    },
}

fn read_json(ptr: *const c_char) -> serde_json::Value {
    if ptr.is_null() {
        return serde_json::Value::Null;
    }
    // SAFETY: see `read_text`.
    let json = unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned();
    serde_json::from_str(&json).unwrap_or(serde_json::Value::Null)
}

fn final_event(v: &serde_json::Value) -> VoskEvent {
    let text = v.get("text").and_then(|t| t.as_str()).unwrap_or_default().to_owned();
    #[allow(clippy::cast_possible_truncation)]
    let words = v
        .get("result")
        .and_then(|r| r.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|w| {
                    Some(VoskWord {
                        word: w.get("word")?.as_str()?.to_owned(),
                        conf: w.get("conf").and_then(serde_json::Value::as_f64).unwrap_or(1.0) as f32,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    VoskEvent::Final { text, words }
}

/// A streaming recognizer (optionally restricted to a grammar: a JSON list
/// of phrases, usually ending in `"[unk]"`), fed chunk by chunk. Used by the
/// barge-in stop-phrase spotter. Not shared between threads (`Send`, not `Sync`).
pub struct VoskStream {
    model: Arc<VoskModel>,
    rec: *mut c_void,
    last_partial: String,
}

// SAFETY: a recognizer may be used from any thread, one call at a time;
// `&mut self` on every call guarantees that. It is freed only in Drop.
unsafe impl Send for VoskStream {}

impl std::fmt::Debug for VoskStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VoskStream").field("model", &self.model.path).finish_non_exhaustive()
    }
}

impl Drop for VoskStream {
    fn drop(&mut self) {
        // SAFETY: `rec` came from vosk_recognizer_new_grm and is freed exactly once.
        unsafe { (self.model.api.recognizer_free)(self.rec) }
    }
}

impl VoskStream {
    /// Opens a streaming recognizer on `model`: restricted to `grammar` (a
    /// JSON array of phrases) or, with `None`, the model's full vocabulary.
    pub fn new(model: &Arc<VoskModel>, sample_rate: u32, grammar: Option<&str>) -> PlatformResult<Self> {
        let api = &model.api;
        let (Some(new_grm), Some(_)) = (api.recognizer_new_grm, api.partial_result) else {
            return Err(PlatformError::Native("this Vosk runtime has no streaming recognizer".into()));
        };
        #[allow(clippy::cast_precision_loss)]
        let rate = sample_rate as c_float;
        let rec = match grammar {
            Some(grammar) => {
                let g = CString::new(grammar).map_err(|_| PlatformError::Native("grammar contains NUL".into()))?;
                // SAFETY: the model handle is valid while `model` lives (the stream keeps it); `g` is NUL-terminated.
                unsafe { new_grm(model.ptr, rate, g.as_ptr()) }
            }
            // SAFETY: as above.
            None => unsafe { (api.recognizer_new)(model.ptr, rate) },
        };
        if rec.is_null() {
            return Err(PlatformError::Native("Vosk could not create a grammar recognizer".into()));
        }
        if let Some(set_words) = api.set_words {
            // SAFETY: `rec` is valid.
            unsafe { set_words(rec, 1) };
        }
        Ok(Self { model: Arc::clone(model), rec, last_partial: String::new() })
    }

    /// Feeds 16-bit mono PCM; returns a final result at an endpoint, a
    /// partial result when it changed, or [`VoskEvent::None`].
    pub fn accept(&mut self, pcm: &[i16]) -> VoskEvent {
        let api = &self.model.api;
        let len = c_int::try_from(pcm.len()).unwrap_or(0);
        // SAFETY: `pcm` is a valid slice of `len` samples for the call; `rec` is valid.
        let done = unsafe { (api.accept_waveform_s)(self.rec, pcm.as_ptr(), len) };
        if done == 1 {
            self.last_partial.clear();
            // SAFETY: as above; the string is copied by read_json.
            return final_event(&read_json(unsafe { (api.result)(self.rec) }));
        }
        let Some(partial) = api.partial_result else { return VoskEvent::None };
        // SAFETY: as above.
        let v = read_json(unsafe { partial(self.rec) });
        let text = v.get("partial").and_then(|t| t.as_str()).unwrap_or_default().to_owned();
        if text == self.last_partial {
            return VoskEvent::None;
        }
        self.last_partial.clone_from(&text);
        VoskEvent::Partial(text)
    }

    /// Ends the utterance in progress and returns its final result.
    pub fn flush(&mut self) -> VoskEvent {
        self.last_partial.clear();
        // SAFETY: `rec` is valid; the string is copied.
        final_event(&read_json(unsafe { (self.model.api.final_result)(self.rec) }))
    }

    /// Drops the utterance in progress.
    pub fn reset(&mut self) {
        self.last_partial.clear();
        if let Some(reset) = self.model.api.reset {
            // SAFETY: `rec` is valid.
            unsafe { reset(self.rec) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_library_is_a_clean_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = VoskLibrary::load(dir.path()).unwrap_err();
        assert!(err.to_string().contains(library_file_name()), "{err}");
    }

    #[test]
    fn result_json_is_parsed() {
        let s = CString::new(r#"{"text" : "привет мир"}"#).unwrap();
        assert_eq!(read_text(s.as_ptr()), "привет мир");
        assert_eq!(read_text(std::ptr::null()), "");
    }

    #[test]
    fn final_result_words_are_parsed() {
        let v: serde_json::Value =
            serde_json::from_str(r#"{"result":[{"conf":0.93,"end":1.1,"start":0.7,"word":"стоп"}],"text":"стоп"}"#)
                .unwrap();
        let VoskEvent::Final { text, words } = final_event(&v) else { panic!("not final") };
        assert_eq!(text, "стоп");
        assert_eq!(words, vec![VoskWord { word: "стоп".into(), conf: 0.93 }]);
    }
}
