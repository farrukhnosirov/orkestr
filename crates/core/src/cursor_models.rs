//! Model backends of cursor capture mode (ADR-0050).
//!
//! - **Subscriptions** (no API key): the Claude Code CLI (`claude -p` with
//!   `stream-json` input, the screenshot as an image content block) and the
//!   Codex CLI (`codex exec --json -i <png>`), signed in with the user's
//!   Claude / ChatGPT subscription.
//! - **API keys** (vault): Anthropic / OpenAI vision models through the
//!   provider stack (`ChatMessage::images`); Gemini too when its key is there.
//! - **mock**: a scripted model, offered only when
//!   `ORKESTR_CURSOR_MOCK_SCRIPT` (a JSON array of actions) or
//!   `ORKESTR_COMPUTER_BACKEND=mock` is set — tests and the sandboxed smoke.
//!
//! All of them answer Orkestr's JSON action prompt
//! ([`orkestr_computer::cursor::model::JsonVisionModel`]); choosing and
//! failing over between them is `orkestr_computer::cursor::select`.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock, Weak};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

use orkestr_computer::cursor::model::{
    ComputerUseModel, JsonVisionModel, ModelError, ModelErrorKind, ScriptedModel, VisionCompleter,
};
use orkestr_computer::cursor::select::CursorModelFactory;
use orkestr_domain::{ModelRef, ProviderKind};
use orkestr_protocol::methods::cursor::{CursorAction, CursorBackendInfo, CursorSettings, CursorSource};
use orkestr_providers::{ChatEvent, ChatMessage, ChatRequest, ImagePart};

use crate::Services;

/// Default Claude Code model alias (fast, good vision).
pub const CLAUDE_DEFAULT: &str = "sonnet";
/// Default Anthropic API model.
pub const ANTHROPIC_DEFAULT: &str = "claude-sonnet-4-5";
/// Default OpenAI API model.
pub const OPENAI_DEFAULT: &str = "gpt-4.1";
/// Default Gemini API model.
pub const GEMINI_DEFAULT: &str = "gemini-2.5-flash";
const CLI_TIMEOUT: Duration = Duration::from_secs(110);

/// The factory core installs into the cursor service.
#[derive(Default)]
pub struct CoreCursorFactory {
    services: OnceLock<Weak<Services>>,
    mock_script: Option<Vec<CursorAction>>,
    work_dir: PathBuf,
}

impl CoreCursorFactory {
    /// Factory; `work_dir` holds the CLIs' working directory and temp images.
    #[must_use]
    pub fn new(work_dir: PathBuf) -> Self {
        let mock_script = std::env::var("ORKESTR_CURSOR_MOCK_SCRIPT")
            .ok()
            .filter(|p| !p.is_empty())
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| serde_json::from_str::<Vec<CursorAction>>(&t).ok())
            .or_else(|| (std::env::var("ORKESTR_COMPUTER_BACKEND").as_deref() == Ok("mock")).then(Vec::new));
        Self { services: OnceLock::new(), mock_script, work_dir }
    }

    /// Connects the services (after they exist).
    pub fn attach(&self, services: &Arc<Services>) {
        let _ = self.services.set(Arc::downgrade(services));
    }

    fn services(&self) -> Option<Arc<Services>> {
        self.services.get().and_then(Weak::upgrade)
    }
}

fn model_for(settings: &CursorSettings, id: &str, default: &str) -> String {
    settings.models.get(id).filter(|m| !m.trim().is_empty()).cloned().unwrap_or_else(|| default.to_owned())
}

fn api_provider(s: &Services, kind: ProviderKind) -> Option<orkestr_providers::ProviderView> {
    s.providers.list().into_iter().find(|v| v.provider.kind == kind && v.provider.enabled && v.has_credential)
}

#[async_trait]
impl CursorModelFactory for CoreCursorFactory {
    async fn backends(&self, settings: &CursorSettings) -> Vec<CursorBackendInfo> {
        let mut out = Vec::new();
        let Some(s) = self.services() else { return out };
        // Claude Code CLI (subscription)
        let exts = s.extensions.enabled_dirs().await.unwrap_or_default();
        let st = s.claude.status(exts, false).await;
        let logged_in = st.auth.as_ref().is_some_and(|a| a.logged_in);
        out.push(CursorBackendInfo {
            id: "claudeCode".into(),
            label: "Claude Code".into(),
            source: CursorSource::Subscriptions,
            available: st.binary.is_some() && logged_in,
            detail: match (&st.binary, &st.auth) {
                (None, _) => "Claude Code CLI not found".into(),
                (Some(_), Some(a)) if a.logged_in => {
                    format!("subscription: {}", a.subscription_type.clone().unwrap_or_else(|| "Claude".into()))
                }
                _ => "not signed in (Claude Code → Sign in)".into(),
            },
            model: model_for(settings, "claudeCode", CLAUDE_DEFAULT),
            consented: false,
        });
        // Codex CLI (ChatGPT subscription)
        let cx = crate::codex::status(&s, false).await;
        out.push(CursorBackendInfo {
            id: "codex".into(),
            label: "Codex (ChatGPT)".into(),
            source: CursorSource::Subscriptions,
            available: cx.binary.is_some() && cx.logged_in,
            detail: match (&cx.binary, cx.logged_in) {
                (None, _) => "Codex CLI not installed".into(),
                (Some(_), false) => "not signed in (Codex → Sign in)".into(),
                (Some(_), true) => "subscription: ChatGPT".into(),
            },
            model: model_for(settings, "codex", ""),
            consented: false,
        });
        // API keys
        for (id, label, kind, default) in [
            ("anthropic", "Anthropic API", ProviderKind::Anthropic, ANTHROPIC_DEFAULT),
            ("openai", "OpenAI API", ProviderKind::OpenAi, OPENAI_DEFAULT),
            ("gemini", "Gemini API", ProviderKind::Gemini, GEMINI_DEFAULT),
        ] {
            let p = api_provider(&s, kind);
            out.push(CursorBackendInfo {
                id: id.into(),
                label: label.into(),
                source: CursorSource::Api,
                available: p.is_some(),
                detail: p.map_or_else(|| "no API key in the vault".into(), |v| format!("key: {}", v.provider.name)),
                model: model_for(settings, id, default),
                consented: false,
            });
        }
        if self.mock_script.is_some() {
            out.push(CursorBackendInfo {
                id: "mock".into(),
                label: "Scripted (test)".into(),
                source: settings.source,
                available: true,
                detail: "ORKESTR_CURSOR_MOCK_SCRIPT".into(),
                model: "scripted".into(),
                consented: true,
            });
        }
        out
    }

    fn build(&self, id: &str, settings: &CursorSettings) -> Option<Arc<dyn ComputerUseModel>> {
        let s = self.services()?;
        let completer: Arc<dyn VisionCompleter> = match id {
            "mock" => return Some(Arc::new(ScriptedModel::actions("mock", self.mock_script.clone()?))),
            "claudeCode" => {
                let st = s.claude_binary_cached()?;
                Arc::new(ClaudeCli {
                    program: st,
                    model: model_for(settings, id, CLAUDE_DEFAULT),
                    cwd: self.work_dir.clone(),
                })
            }
            "codex" => {
                let program = s.codex.locate(None).map(|(p, _)| p)?;
                let model = model_for(settings, id, "");
                Arc::new(CodexCli { program, model, cwd: self.work_dir.clone() })
            }
            "anthropic" | "openai" | "gemini" => {
                let (id, kind, default): (&'static str, _, _) = match id {
                    "anthropic" => ("anthropic", ProviderKind::Anthropic, ANTHROPIC_DEFAULT),
                    "openai" => ("openai", ProviderKind::OpenAi, OPENAI_DEFAULT),
                    _ => ("gemini", ProviderKind::Gemini, GEMINI_DEFAULT),
                };
                let p = api_provider(&s, kind)?;
                Arc::new(ApiVision {
                    id,
                    services: Arc::downgrade(&s),
                    model: ModelRef { provider_id: p.provider.id, model: model_for(settings, id, default) },
                })
            }
            _ => return None,
        };
        Some(Arc::new(JsonVisionModel::new(completer)))
    }
}

impl Services {
    /// The Claude Code binary from the last status check (cursor mode).
    #[must_use]
    pub fn claude_binary_cached(&self) -> Option<PathBuf> {
        self.claude.cached_binary()
    }
}

// ── Claude Code CLI ───────────────────────────────────────────────────────

struct ClaudeCli {
    program: PathBuf,
    model: String,
    cwd: PathBuf,
}

/// Arguments of one Claude Code call (no tools, no session, no project settings).
#[must_use]
pub fn claude_args(model: &str, system: &str) -> Vec<String> {
    [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--tools",
        "",
        "--no-session-persistence",
        "--setting-sources",
        "",
        "--strict-mcp-config",
        "--model",
        model,
        "--system-prompt",
        system,
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect()
}

/// The stream-json user line: the screenshot, then the text.
#[must_use]
pub fn claude_input(user: &str, png_base64: &str) -> String {
    let mut content = Vec::new();
    if !png_base64.is_empty() {
        content.push(json!({"type":"image","source":{"type":"base64","media_type":"image/png","data":png_base64}}));
    }
    content.push(json!({"type":"text","text":user}));
    json!({"type":"user","message":{"role":"user","content":content}}).to_string()
}

/// Reads the `result` event of a `stream-json` transcript.
///
/// # Errors
/// The CLI reported an error (classified) or produced no result.
pub fn claude_result(stdout: &str, stderr: &str) -> Result<String, ModelError> {
    for line in stdout.lines().rev() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if v["type"] == "result" {
            let text = v["result"].as_str().unwrap_or_default().to_owned();
            if v["is_error"].as_bool().unwrap_or(false) || v["subtype"] != "success" {
                let msg = if text.is_empty() { v["subtype"].as_str().unwrap_or("error").to_owned() } else { text };
                return Err(ModelError::classified(format!("Claude Code: {msg}")));
            }
            return Ok(text);
        }
    }
    let tail: String = stderr.lines().rev().take(4).collect::<Vec<_>>().join(" | ");
    let mut e = ModelError::classified(format!("Claude Code gave no result: {tail}"));
    if e.kind == ModelErrorKind::Other {
        e.kind = ModelErrorKind::Unavailable;
    }
    Err(e)
}

async fn run_cli(
    mut cmd: tokio::process::Command,
    stdin: String,
    cancel: &CancellationToken,
) -> Result<(String, String), ModelError> {
    let mut child =
        cmd.spawn().map_err(|e| ModelError::new(ModelErrorKind::Unavailable, format!("cannot start the CLI: {e}")))?;
    if let Some(mut i) = child.stdin.take() {
        let _ = i.write_all(stdin.as_bytes()).await;
        let _ = i.write_all(b"\n").await;
        drop(i);
    }
    let mut out = child.stdout.take();
    let mut err = child.stderr.take();
    let read = async {
        let mut o = Vec::new();
        let mut e = Vec::new();
        let (a, b) = tokio::join!(
            async {
                if let Some(s) = out.as_mut() {
                    let _ = s.read_to_end(&mut o).await;
                }
            },
            async {
                if let Some(s) = err.as_mut() {
                    let _ = s.read_to_end(&mut e).await;
                }
            }
        );
        let ((), ()) = (a, b);
        let _ = child.wait().await;
        (String::from_utf8_lossy(&o).into_owned(), String::from_utf8_lossy(&e).into_owned())
    };
    tokio::select! {
        r = tokio::time::timeout(CLI_TIMEOUT, read) => r.map_err(|_| ModelError::new(ModelErrorKind::Network, "the CLI timed out")),
        () = cancel.cancelled() => Err(ModelError::cancelled()),
    }
}

#[async_trait]
impl VisionCompleter for ClaudeCli {
    fn id(&self) -> &str {
        "claudeCode"
    }
    fn label(&self) -> String {
        format!("Claude Code · {}", self.model)
    }
    async fn complete(
        &self,
        system: &str,
        user: &str,
        png: &str,
        cancel: &CancellationToken,
    ) -> Result<String, ModelError> {
        let _ = std::fs::create_dir_all(&self.cwd);
        let mut cmd = orkestr_platform::process::stdio_command(&self.program, &claude_args(&self.model, system));
        cmd.current_dir(&self.cwd);
        let (stdout, stderr) = run_cli(cmd, claude_input(user, png), cancel).await?;
        claude_result(&stdout, &stderr)
    }
}

// ── Codex CLI ─────────────────────────────────────────────────────────────

struct CodexCli {
    program: PathBuf,
    model: String,
    cwd: PathBuf,
}

/// Arguments of one `codex exec` call (read-only sandbox, no session).
#[must_use]
pub fn codex_args(model: &str, image: &str) -> Vec<String> {
    let mut v: Vec<String> = ["exec", "--json", "-s", "read-only", "--skip-git-repo-check", "--ephemeral"]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    if !model.is_empty() {
        v.push("-m".into());
        v.push(model.to_owned());
    }
    v.push("-i".into());
    v.push(image.to_owned());
    v.push("-".into());
    v
}

/// The last agent message of a `codex exec --json` transcript.
///
/// # Errors
/// An `error` / `turn.failed` event (classified) or no message.
pub fn codex_result(stdout: &str, stderr: &str) -> Result<String, ModelError> {
    let mut last = None;
    let mut error = None;
    for line in stdout.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        match v["type"].as_str() {
            Some("item.completed") if v["item"]["type"] == "agent_message" => {
                last = v["item"]["text"].as_str().map(str::to_owned);
            }
            Some("error") => error = v["message"].as_str().map(str::to_owned),
            Some("turn.failed") => error = v["error"]["message"].as_str().map(str::to_owned).or(error),
            _ => {}
        }
    }
    if let Some(t) = last {
        return Ok(t);
    }
    let msg = error.unwrap_or_else(|| stderr.lines().rev().take(4).collect::<Vec<_>>().join(" | "));
    let mut e = ModelError::classified(format!("Codex: {msg}"));
    if e.kind == ModelErrorKind::Other {
        e.kind = ModelErrorKind::Unavailable;
    }
    Err(e)
}

#[async_trait]
impl VisionCompleter for CodexCli {
    fn id(&self) -> &str {
        "codex"
    }
    fn label(&self) -> String {
        if self.model.is_empty() {
            "Codex".into()
        } else {
            format!("Codex · {}", self.model)
        }
    }
    async fn complete(
        &self,
        system: &str,
        user: &str,
        png: &str,
        cancel: &CancellationToken,
    ) -> Result<String, ModelError> {
        use base64::Engine as _;
        let _ = std::fs::create_dir_all(&self.cwd);
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let file = self.cwd.join(format!("shot-{}-{n}.png", std::process::id()));
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(png)
            .map_err(|e| ModelError::new(ModelErrorKind::Other, format!("bad image: {e}")))?;
        std::fs::write(&file, bytes).map_err(|e| ModelError::new(ModelErrorKind::Other, format!("temp image: {e}")))?;
        let mut cmd = orkestr_platform::process::stdio_command(
            &self.program,
            &codex_args(&self.model, &file.display().to_string()),
        );
        cmd.current_dir(&self.cwd);
        let prompt = format!("{system}\n\n---\n\n{user}\n\nDo not run any commands; answer with the JSON object only.");
        let r = run_cli(cmd, prompt, cancel).await;
        let _ = std::fs::remove_file(&file);
        let (stdout, stderr) = r?;
        codex_result(&stdout, &stderr)
    }
}

// ── API keys (provider stack) ─────────────────────────────────────────────

struct ApiVision {
    id: &'static str,
    services: Weak<Services>,
    model: ModelRef,
}

#[async_trait]
impl VisionCompleter for ApiVision {
    fn id(&self) -> &str {
        self.id
    }
    fn label(&self) -> String {
        self.model.model.clone()
    }
    async fn complete(
        &self,
        system: &str,
        user: &str,
        png: &str,
        cancel: &CancellationToken,
    ) -> Result<String, ModelError> {
        let s = self
            .services
            .upgrade()
            .ok_or_else(|| ModelError::new(ModelErrorKind::Unavailable, "core is shutting down"))?;
        let mut msg = ChatMessage::user(user);
        if !png.is_empty() {
            msg.images.push(ImagePart { media_type: "image/png".into(), data_base64: png.to_owned() });
        }
        let mut req = ChatRequest::new(self.model.model.clone(), vec![msg]);
        req.system = Some(system.to_owned());
        req.max_output_tokens = Some(800);
        req.temperature = Some(0.0);
        let mut stream = s.providers.chat(vec![self.model.clone()], req, None, cancel.clone());
        let mut text = String::new();
        while let Some(ev) = stream.next().await {
            match ev {
                ChatEvent::TextDelta(t) => text.push_str(&t),
                ChatEvent::Switched { .. } => text.clear(),
                ChatEvent::Done { .. } => return Ok(text),
                ChatEvent::Error(e) => {
                    if cancel.is_cancelled() {
                        return Err(ModelError::cancelled());
                    }
                    return Err(ModelError::classified(format!("{}: {e}", self.id)));
                }
                ChatEvent::Usage(_) | ChatEvent::ToolCall { .. } => {}
            }
        }
        Err(ModelError::new(ModelErrorKind::Network, "the model stream ended unexpectedly"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_transcripts_are_parsed() {
        let ok = "{\"type\":\"system\"}\n{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"{\\\"x\\\":1}\"}";
        assert_eq!(claude_result(ok, "").unwrap(), "{\"x\":1}");
        let lim = "{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":true,\"result\":\"Claude AI usage limit reached|1730000000\"}";
        assert_eq!(claude_result(lim, "").unwrap_err().kind, ModelErrorKind::Limit);
        let auth = "{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":true,\"result\":\"Invalid API key · Please run /login\"}";
        assert_eq!(claude_result(auth, "").unwrap_err().kind, ModelErrorKind::Auth);
        assert_eq!(claude_result("", "boom").unwrap_err().kind, ModelErrorKind::Unavailable);
        let line = claude_input("hi", "AAAA");
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["message"]["content"][0]["source"]["media_type"], "image/png");
        assert_eq!(v["message"]["content"][1]["text"], "hi");
        let a = claude_args("sonnet", "sys");
        assert!(a.windows(2).any(|w| w[0] == "--tools" && w[1].is_empty()));
    }

    #[test]
    fn codex_transcripts_are_parsed() {
        let ok = "{\"type\":\"turn.started\"}\n{\"type\":\"item.completed\",\"item\":{\"id\":\"i\",\"type\":\"agent_message\",\"text\":\"{}\"}}";
        assert_eq!(codex_result(ok, "").unwrap(), "{}");
        let lim = "{\"type\":\"error\",\"message\":\"You've hit your usage limit.\"}\n{\"type\":\"turn.failed\",\"error\":{\"message\":\"You've hit your usage limit.\"}}";
        assert_eq!(codex_result(lim, "").unwrap_err().kind, ModelErrorKind::Limit);
        let a = codex_args("", "C:/x.png");
        assert_eq!(a.last().unwrap(), "-");
        assert!(!a.contains(&"-m".to_owned()));
    }
}
