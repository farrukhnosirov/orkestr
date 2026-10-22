//! Persistent Claude Code processes for chat-style calls (AI Manager,
//! reviews) on the Claude Code subscription provider.
//!
//! A chat request carries the whole conversation every turn. Spawning
//! `claude -p` per turn re-reads it all and pays the CLI start-up each time;
//! instead one long-lived `claude --input-format stream-json` process (the
//! same stream-json protocol as the Claude Code panel's sessions) is kept
//! per conversation and only the new messages are sent to it.
//!
//! The pool needs no conversation id: a process remembers the messages it
//! has been given; a request whose conversation starts with exactly those
//! messages, followed by the assistant's answer and new user / tool
//! messages, continues on that process. Anything else (a different system
//! prompt, tools, model, an edited history) starts a fresh process with the
//! full conversation. A process that died (crash, kill, idle timeout) is
//! restarted transparently: the turn is replayed on a new process with the
//! full conversation, once, if it had not produced any output yet.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::claude_code::ClaudeStreamParser;
use super::cli::{self, CliStreamParser};
use crate::http::{drive, Emitter};
use crate::types::{ChatEvent, ChatMessage, ChatRequest, CodingAgentEvent, CodingAgentStream, ProviderError, Role};

/// Idle processes kept at most (oldest are stopped first).
pub const MAX_IDLE: usize = 4;
/// An idle process is stopped after this long.
pub const IDLE_TTL: Duration = Duration::from_secs(15 * 60);

/// A running CLI process: stdin lines in, stdout lines out. Dropping it
/// stops the process (tree).
pub struct ChatProc {
    /// Lines to the process's stdin.
    pub stdin: mpsc::UnboundedSender<String>,
    /// Lines from its stdout (`None` = the process ended).
    pub stdout: mpsc::UnboundedReceiver<String>,
    /// Stops the process when dropped.
    pub _guard: Box<dyn Send + Sync>,
}

/// Starts processes (the real CLI, or a fake in tests).
pub trait ProcSpawner: Send + Sync {
    /// Starts `program args` in `cwd`.
    fn spawn(&self, program: &Path, args: &[String], cwd: &Path) -> Result<ChatProc, ProviderError>;
}

/// Spawns the real CLI (allow-listed environment, no console window).
#[derive(Debug, Default)]
pub struct CliSpawner;

impl ProcSpawner for CliSpawner {
    fn spawn(&self, program: &Path, args: &[String], cwd: &Path) -> Result<ChatProc, ProviderError> {
        let mut cmd = cli::command(program, args);
        cmd.current_dir(cwd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ProviderError::Auth(format!("`{}` not found — install the official CLI", program.display()))
            } else {
                ProviderError::Server(format!("failed to start `{}`: {e}", program.display()))
            }
        })?;
        let guard = cli::TreeGuard(child.id());
        let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            return Err(ProviderError::Server("the CLI has no stdio".into()));
        };
        let (in_tx, mut in_rx) = mpsc::unbounded_channel::<String>();
        let (out_tx, out_rx) = mpsc::unbounded_channel::<String>();
        tokio::spawn(async move {
            while let Some(line) = in_rx.recv().await {
                if stdin.write_all(line.as_bytes()).await.is_err()
                    || stdin.write_all(b"\n").await.is_err()
                    || stdin.flush().await.is_err()
                {
                    break;
                }
            }
        });
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if out_tx.send(line).is_err() {
                    break;
                }
            }
            let _ = child.wait().await;
        });
        Ok(ChatProc { stdin: in_tx, stdout: out_rx, _guard: Box::new(guard) })
    }
}

/// One message as the pool compares it.
fn fingerprint(m: &ChatMessage) -> String {
    let calls: Vec<String> = m.tool_calls.iter().map(|c| format!("{}:{}:{}", c.id, c.name, c.arguments)).collect();
    format!("{:?}|{}|{}|{}|{}", m.role, m.tool_call_id.as_deref().unwrap_or(""), m.is_error, m.content, calls.join(";"))
}

/// What a process is bound to: model, instructions, tool catalogue, format.
fn session_key(req: &ChatRequest, tool_protocol: &str) -> String {
    format!(
        "{}\u{1}{}\u{1}{}\u{1}{:?}",
        req.model,
        req.system_prompt().unwrap_or_default(),
        tool_protocol,
        req.response_format
    )
}

/// New messages after a turn, as one user message for the process.
#[must_use]
pub fn render_tail(tail: &[&ChatMessage], req: &ChatRequest) -> String {
    let mut out = String::new();
    if let [only] = tail {
        if only.role == Role::User {
            out.push_str(&only.content);
        }
    }
    if out.is_empty() {
        for m in tail {
            match (m.role, m.tool_call_id.as_deref()) {
                (Role::Tool, Some(id)) => {
                    let err = if m.is_error { " (error)" } else { "" };
                    out.push_str(&format!("Tool result for call {id}{err}: {}\n", m.content));
                }
                (Role::Assistant, _) => out.push_str(&format!("Assistant: {}\n", m.content)),
                _ => out.push_str(&format!("{}\n", m.content)),
            }
        }
    }
    // The answer format is restated every turn (as the one-shot prompt does).
    let format = cli::render_prompt(&ChatRequest {
        system: None,
        messages: vec![ChatMessage::user(String::new())],
        ..req.clone()
    });
    if !format.trim().is_empty() {
        out.push_str("\n\n");
        out.push_str(format.trim());
    }
    out
}

struct Session {
    key: String,
    /// Conversation (fingerprints) the process has answered.
    seen: Vec<String>,
    proc: ChatProc,
    last_used: Instant,
}

/// The pool of persistent chat processes of one Claude Code provider.
pub struct ChatPool {
    spawner: Arc<dyn ProcSpawner>,
    idle: Mutex<Vec<Session>>,
    /// Extra CLI arguments (model / tool restrictions are added per request).
    base_args: Vec<String>,
}

impl std::fmt::Debug for ChatPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChatPool").field("idle", &self.idle.lock().len()).finish()
    }
}

/// How a turn continues a process.
enum Plan {
    /// A process that saw `seen.len()` messages; send the tail.
    Continue(Session, usize),
    /// A new process with the whole conversation.
    Fresh,
}

impl ChatPool {
    /// Creates a pool; `base_args` are appended to every process.
    #[must_use]
    pub fn new(spawner: Arc<dyn ProcSpawner>, base_args: Vec<String>) -> Self {
        Self { spawner, idle: Mutex::new(Vec::new()), base_args }
    }

    /// Idle processes (tests, diagnostics).
    #[must_use]
    pub fn idle_count(&self) -> usize {
        self.idle.lock().len()
    }

    /// Stops every idle process.
    pub fn clear(&self) {
        self.idle.lock().clear();
    }

    fn take(&self, key: &str, convo: &[String]) -> Plan {
        let mut idle = self.idle.lock();
        idle.retain(|s| s.last_used.elapsed() < IDLE_TTL);
        // The conversation must be: what the process saw + its answer + new messages.
        let pos = idle.iter().position(|s| {
            s.key == key
                && convo.len() > s.seen.len() + 1
                && convo[..s.seen.len()] == s.seen[..]
                && convo[s.seen.len()].starts_with("Assistant|")
        });
        match pos {
            Some(i) => {
                let s = idle.swap_remove(i);
                let n = s.seen.len() + 1;
                Plan::Continue(s, n)
            }
            None => Plan::Fresh,
        }
    }

    fn put_back(&self, mut s: Session) {
        s.last_used = Instant::now();
        let mut idle = self.idle.lock();
        idle.push(s);
        while idle.len() > MAX_IDLE {
            if let Some(oldest) = idle.iter().enumerate().min_by_key(|(_, s)| s.last_used).map(|(i, _)| i) {
                idle.swap_remove(oldest);
            }
        }
    }

    /// Runs one chat turn. `first_prompt` is the full one-shot prompt (used
    /// for a new process); `tool_protocol` is appended to it.
    pub fn turn(
        self: &Arc<Self>,
        program: PathBuf,
        cwd: PathBuf,
        request: ChatRequest,
        first_prompt: String,
        tool_protocol: String,
        cancel: CancellationToken,
    ) -> CodingAgentStream {
        let pool = Arc::clone(self);
        drive(cancel.clone(), move |em: Emitter<CodingAgentEvent>| async move {
            let key = session_key(&request, &tool_protocol);
            let convo_msgs: Vec<&ChatMessage> = request.conversation().collect();
            let convo: Vec<String> = convo_msgs.iter().map(|m| fingerprint(m)).collect();
            let mut plan = pool.take(&key, &convo);
            loop {
                let (session, text, reused) = match plan {
                    Plan::Continue(s, from) => {
                        let tail: Vec<&ChatMessage> = convo_msgs[from..].to_vec();
                        let text = render_tail(&tail, &request);
                        (s, text, true)
                    }
                    Plan::Fresh => {
                        let mut args = pool.base_args.clone();
                        if !request.model.trim().is_empty() && request.model != "default" {
                            args.push("--model".into());
                            args.push(request.model.clone());
                        }
                        let proc = pool.spawner.spawn(&program, &args, &cwd)?;
                        let s = Session { key: key.clone(), seen: Vec::new(), proc, last_used: Instant::now() };
                        (s, format!("{first_prompt}{tool_protocol}"), false)
                    }
                };
                match run_turn(session, &text, &em, &cancel).await? {
                    TurnEnd::Answered(mut s) => {
                        s.seen.clone_from(&convo);
                        pool.put_back(s);
                        return Ok(());
                    }
                    TurnEnd::Failed => return Ok(()),
                    TurnEnd::Died { output } => {
                        if reused && !output {
                            // Restart: replay the turn on a new process.
                            tracing::info!("Claude Code chat process ended; restarting it");
                            plan = Plan::Fresh;
                            continue;
                        }
                        return Err(ProviderError::Server("the Claude Code process ended during the answer".into()));
                    }
                }
            }
        })
    }
}

enum TurnEnd {
    /// The turn finished; the process can be reused.
    Answered(Session),
    /// A terminal error was emitted (the process is dropped).
    Failed,
    /// The process ended before the turn finished.
    Died { output: bool },
}

async fn run_turn(
    mut s: Session,
    text: &str,
    em: &Emitter<CodingAgentEvent>,
    cancel: &CancellationToken,
) -> Result<TurnEnd, ProviderError> {
    if s.proc.stdin.send(crate::claude_code_session::control::user_message(text)).is_err() {
        return Ok(TurnEnd::Died { output: false });
    }
    let mut parser = ClaudeStreamParser::default();
    let mut output = false;
    loop {
        let line = tokio::select! {
            () = cancel.cancelled() => return Err(ProviderError::Cancelled),
            l = tokio::time::timeout(cli::CLI_IDLE_TIMEOUT, s.proc.stdout.recv()) => match l {
                Ok(l) => l,
                Err(_) => {
                    return Err(ProviderError::Server(format!(
                        "Claude Code produced no output for {}s",
                        cli::CLI_IDLE_TIMEOUT.as_secs()
                    )))
                }
            },
        };
        let Some(line) = line else { return Ok(TurnEnd::Died { output }) };
        for ev in parser.line(&line) {
            match ev {
                CodingAgentEvent::Chat(ChatEvent::Done { .. }) => {
                    em.emit(ev).await?;
                    return Ok(TurnEnd::Answered(s));
                }
                CodingAgentEvent::Chat(ChatEvent::Error(_)) => {
                    em.emit(ev).await?;
                    return Ok(TurnEnd::Failed);
                }
                CodingAgentEvent::Chat(ChatEvent::TextDelta(_)) => {
                    output = true;
                    em.emit(ev).await?;
                }
                other => em.emit(other).await?,
            }
        }
    }
}

/// Collects a turn's text (tests).
#[cfg(test)]
async fn collect(stream: CodingAgentStream) -> (String, Vec<CodingAgentEvent>) {
    use futures::StreamExt;
    let events: Vec<CodingAgentEvent> = stream.collect().await;
    let text = events
        .iter()
        .filter_map(|e| match e {
            CodingAgentEvent::Chat(ChatEvent::TextDelta(t)) => Some(t.as_str()),
            _ => None,
        })
        .collect();
    (text, events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A fake `claude` in stream-json mode: answers every user message with
    /// "reply N: <text>" (N counts turns of that process); a message
    /// containing "CRASH" ends the process without an answer.
    #[derive(Default)]
    struct FakeClaude {
        spawned: AtomicUsize,
        inputs: Arc<Mutex<Vec<(usize, String)>>>,
    }

    impl ProcSpawner for FakeClaude {
        fn spawn(&self, _program: &Path, _args: &[String], _cwd: &Path) -> Result<ChatProc, ProviderError> {
            let n = self.spawned.fetch_add(1, Ordering::SeqCst) + 1;
            let (in_tx, mut in_rx) = mpsc::unbounded_channel::<String>();
            let (out_tx, out_rx) = mpsc::unbounded_channel::<String>();
            let inputs = Arc::clone(&self.inputs);
            tokio::spawn(async move {
                let mut turn = 0;
                while let Some(line) = in_rx.recv().await {
                    let v: serde_json::Value = serde_json::from_str(&line).unwrap();
                    let text =
                        v.pointer("/message/content/0/text").and_then(serde_json::Value::as_str).unwrap().to_owned();
                    inputs.lock().push((n, text.clone()));
                    if text.contains("CRASH") {
                        return; // drops out_tx: stdout closes
                    }
                    turn += 1;
                    let reply = format!("reply {turn}");
                    let msg =
                        serde_json::json!({"type":"assistant","message":{"content":[{"type":"text","text":reply}]}});
                    let _ = out_tx.send(msg.to_string());
                    let _ = out_tx.send(
                        r#"{"type":"result","subtype":"success","is_error":false,"result":"","usage":{}}"#.into(),
                    );
                }
            });
            Ok(ChatProc { stdin: in_tx, stdout: out_rx, _guard: Box::new(()) })
        }
    }

    fn pool(fake: &Arc<FakeClaude>) -> Arc<ChatPool> {
        Arc::new(ChatPool::new(Arc::clone(fake) as Arc<dyn ProcSpawner>, Vec::new()))
    }

    async fn ask(pool: &Arc<ChatPool>, messages: Vec<ChatMessage>) -> String {
        let mut req = ChatRequest::new("sonnet", messages);
        req.system = Some("You are the manager.".into());
        let first = cli::render_prompt(&req);
        let stream =
            pool.turn(PathBuf::from("claude"), PathBuf::from("."), req, first, String::new(), CancellationToken::new());
        collect(stream).await.0
    }

    #[tokio::test]
    async fn one_process_per_conversation_and_only_new_messages_are_sent() {
        let fake = Arc::new(FakeClaude::default());
        let pool = pool(&fake);
        let mut convo = vec![ChatMessage::user("plan the release")];
        assert_eq!(ask(&pool, convo.clone()).await, "reply 1");
        convo.push(ChatMessage::assistant("reply 1"));
        convo.push(ChatMessage::user("now split it into tasks"));
        assert_eq!(ask(&pool, convo.clone()).await, "reply 2", "the same process answered");
        assert_eq!(fake.spawned.load(Ordering::SeqCst), 1);
        let inputs = fake.inputs.lock().clone();
        assert!(inputs[0].1.contains("You are the manager.") && inputs[0].1.contains("plan the release"));
        assert_eq!(inputs[1].1, "now split it into tasks", "only the new message");
        // Another conversation gets its own process.
        assert_eq!(ask(&pool, vec![ChatMessage::user("something else")]).await, "reply 1");
        assert_eq!(fake.spawned.load(Ordering::SeqCst), 2);
        assert_eq!(pool.idle_count(), 2);
    }

    #[tokio::test]
    async fn a_crashed_process_is_restarted_with_the_full_conversation() {
        let fake = Arc::new(FakeClaude::default());
        let pool = pool(&fake);
        let mut convo = vec![ChatMessage::user("hello")];
        assert_eq!(ask(&pool, convo.clone()).await, "reply 1");
        convo.push(ChatMessage::assistant("reply 1"));
        convo.push(ChatMessage::user("CRASH please"));
        // The reused process dies without output → replayed on a new one
        // (which also "crashes" on the same text: then the error surfaces).
        let mut req = ChatRequest::new("sonnet", convo.clone());
        req.system = Some("You are the manager.".into());
        let first = cli::render_prompt(&req);
        let (_, events) = collect(pool.turn(
            PathBuf::from("claude"),
            PathBuf::from("."),
            req,
            first,
            String::new(),
            CancellationToken::new(),
        ))
        .await;
        assert_eq!(fake.spawned.load(Ordering::SeqCst), 2, "restarted once");
        assert!(matches!(events.last(), Some(CodingAgentEvent::Chat(ChatEvent::Error(ProviderError::Server(_))))));
        let inputs = fake.inputs.lock().clone();
        assert!(inputs[2].1.contains("Conversation so far") && inputs[2].1.contains("hello"), "{inputs:?}");

        // After the crash the conversation continues on a fresh process.
        convo.pop();
        convo.push(ChatMessage::user("are you there?"));
        assert_eq!(ask(&pool, convo).await, "reply 1");
        assert_eq!(fake.spawned.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn an_edited_history_starts_a_new_process() {
        let fake = Arc::new(FakeClaude::default());
        let pool = pool(&fake);
        assert_eq!(ask(&pool, vec![ChatMessage::user("a")]).await, "reply 1");
        let edited = vec![ChatMessage::user("b"), ChatMessage::assistant("reply 1"), ChatMessage::user("c")];
        assert_eq!(ask(&pool, edited).await, "reply 1");
        assert_eq!(fake.spawned.load(Ordering::SeqCst), 2);
    }
}
