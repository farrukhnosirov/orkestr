//! The Codex panel's shared `codex app-server` process (ADR-0047): one
//! long-lived process hosts every panel session (one thread each), answers
//! status / limits / models / threads queries and runs the ChatGPT sign-in.
//!
//! Approval requests are classified by a [`CodexPolicy`] (the core plugs in
//! the Dangerous Action Guard + audit log): forbidden ones are declined
//! without asking, everything else becomes an inline
//! [`CodexEvent::ApprovalRequest`] answered with [`CodexHub::respond`].
//! A process that dies is restarted on the next call; its sessions end.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use orkestr_domain::{CodexSessionId, ProjectId};
use orkestr_protocol::methods::claude_code::{ClaudeRisk, ClaudeRiskLevel};
use orkestr_protocol::methods::codex::{
    CodexAccount, CodexAccountPayload, CodexApprovalDecision, CodexApprovalKind, CodexApprovalMode, CodexEvent,
    CodexLimits, CodexLoginMode, CodexLoginPhase, CodexLoginState, CodexModelOption, CodexSessionInfo,
    CodexSessionState, CodexThread,
};
use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio::sync::mpsc;

use super::conn::{AppServer, Inbound, THREAD_TIMEOUT};
use super::map::{self, PanelMapper};
use crate::adapters::claude_chat::ProcSpawner;
use crate::claude_code_session::PermissionOutcome;
use crate::types::ProviderError;

/// Context of an approval decision.
#[derive(Debug, Clone)]
pub struct CodexPolicyContext {
    /// Session.
    pub session_id: CodexSessionId,
    /// Project.
    pub project_id: ProjectId,
    /// Working directory.
    pub cwd: PathBuf,
}

/// Classifies and records approval requests (the core: guard + audit).
#[async_trait]
pub trait CodexPolicy: Send + Sync {
    /// Risk of a request (`Bash {command}` / `Edit {file_path}`).
    fn assess(&self, ctx: &CodexPolicyContext, tool: &str, input: &Value) -> Option<ClaudeRisk>;
    /// Records a decision.
    async fn record(
        &self,
        ctx: &CodexPolicyContext,
        tool: &str,
        input: &Value,
        risk: Option<&ClaudeRisk>,
        outcome: PermissionOutcome,
    );
}

/// Flags nothing, records nothing (tests).
#[derive(Debug, Default)]
pub struct NoCodexPolicy;

#[async_trait]
impl CodexPolicy for NoCodexPolicy {
    fn assess(&self, _: &CodexPolicyContext, _: &str, _: &Value) -> Option<ClaudeRisk> {
        None
    }
    async fn record(&self, _: &CodexPolicyContext, _: &str, _: &Value, _: Option<&ClaudeRisk>, _: PermissionOutcome) {}
}

/// What the hub reports to the core.
#[derive(Debug, Clone, PartialEq)]
pub enum HubEvent {
    /// A session changed state.
    Status(CodexSessionInfo),
    /// A chat item.
    Item {
        /// Session.
        session_id: CodexSessionId,
        /// Project.
        project_id: ProjectId,
        /// Working directory.
        cwd: PathBuf,
        /// Item.
        event: CodexEvent,
    },
    /// Sign-in / account change.
    Account(CodexAccountPayload),
    /// Plan limits changed.
    Limits(CodexLimits),
}

struct Pending {
    rpc_id: Value,
    tool: String,
    input: Value,
    risk: Option<ClaudeRisk>,
}

struct Sess {
    info: CodexSessionInfo,
    mapper: PanelMapper,
    pending: HashMap<String, Pending>,
}

#[derive(Default)]
struct State {
    sessions: HashMap<CodexSessionId, Sess>,
    login: Option<CodexLoginState>,
    limits: Option<CodexLimits>,
    account: Option<(bool, Option<CodexAccount>)>,
}

/// The panel hub.
pub struct CodexHub {
    spawner: Arc<dyn ProcSpawner>,
    program: Mutex<Option<PathBuf>>,
    conn: tokio::sync::Mutex<Option<Arc<AppServer>>>,
    state: Arc<Mutex<State>>,
    events: mpsc::UnboundedSender<HubEvent>,
    policy: Arc<dyn CodexPolicy>,
}

impl std::fmt::Debug for CodexHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodexHub").field("sessions", &self.state.lock().sessions.len()).finish_non_exhaustive()
    }
}

fn err(msg: impl Into<String>) -> ProviderError {
    ProviderError::BadRequest(msg.into())
}

fn sandbox_policy(mode: CodexApprovalMode) -> Value {
    match mode {
        CodexApprovalMode::ReadOnly => json!({ "type": "readOnly" }),
        CodexApprovalMode::Ask | CodexApprovalMode::Auto => json!({ "type": "workspaceWrite" }),
    }
}

fn account_of(v: &Value) -> (bool, Option<CodexAccount>) {
    match v.get("account").filter(|a| !a.is_null()) {
        Some(a) => (
            true,
            Some(CodexAccount {
                kind: a.get("type").and_then(Value::as_str).unwrap_or("chatgpt").to_owned(),
                email: a.get("email").and_then(Value::as_str).map(str::to_owned),
                plan: a.get("planType").and_then(Value::as_str).map(str::to_owned),
            }),
        ),
        None => (false, None),
    }
}

impl CodexHub {
    /// Creates the hub; events go to `events`.
    #[must_use]
    pub fn new(
        spawner: Arc<dyn ProcSpawner>,
        events: mpsc::UnboundedSender<HubEvent>,
        policy: Arc<dyn CodexPolicy>,
    ) -> Self {
        Self {
            spawner,
            program: Mutex::new(None),
            conn: tokio::sync::Mutex::new(None),
            state: Arc::default(),
            events,
            policy,
        }
    }

    /// Sets the binary (a change restarts the process on the next call).
    pub fn set_program(&self, program: Option<PathBuf>) {
        let mut p = self.program.lock();
        if *p != program {
            *p = program;
            if let Ok(mut c) = self.conn.try_lock() {
                *c = None;
            }
        }
    }

    /// The binary in use.
    #[must_use]
    pub fn program(&self) -> Option<PathBuf> {
        self.program.lock().clone()
    }

    /// The running app-server (started when needed).
    pub async fn conn(&self) -> Result<Arc<AppServer>, ProviderError> {
        let mut guard = self.conn.lock().await;
        if let Some(c) = guard.as_ref().filter(|c| c.is_alive()) {
            return Ok(Arc::clone(c));
        }
        let program =
            self.program().ok_or_else(|| ProviderError::Auth("Codex CLI (`codex`) is not installed".into()))?;
        let (srv, rx) = AppServer::start(&*self.spawner, &program, &std::env::temp_dir()).await?;
        tokio::spawn(route(
            Arc::downgrade(&srv),
            rx,
            Arc::clone(&self.state),
            self.events.clone(),
            Arc::clone(&self.policy),
        ));
        *guard = Some(Arc::clone(&srv));
        Ok(srv)
    }

    fn emit(&self, ev: HubEvent) {
        let _ = self.events.send(ev);
    }

    fn item(&self, info: &CodexSessionInfo, event: CodexEvent) {
        self.emit(HubEvent::Item {
            session_id: info.session_id,
            project_id: info.project_id,
            cwd: PathBuf::from(&info.cwd),
            event,
        });
    }

    fn update(
        &self,
        id: CodexSessionId,
        f: impl FnOnce(&mut CodexSessionInfo),
    ) -> Result<CodexSessionInfo, ProviderError> {
        let info = {
            let mut st = self.state.lock();
            let s = st.sessions.get_mut(&id).ok_or_else(|| err("unknown Codex session"))?;
            f(&mut s.info);
            s.info.clone()
        };
        self.emit(HubEvent::Status(info.clone()));
        Ok(info)
    }

    /// A live session.
    pub fn get(&self, id: CodexSessionId) -> Result<CodexSessionInfo, ProviderError> {
        self.state.lock().sessions.get(&id).map(|s| s.info.clone()).ok_or_else(|| err("unknown Codex session"))
    }

    /// Live sessions (of a project).
    #[must_use]
    pub fn sessions(&self, project: Option<ProjectId>) -> Vec<CodexSessionInfo> {
        let mut v: Vec<CodexSessionInfo> = self
            .state
            .lock()
            .sessions
            .values()
            .filter(|s| project.is_none_or(|p| s.info.project_id == p))
            .map(|s| s.info.clone())
            .collect();
        v.sort_by_key(|s| s.started_at);
        v
    }

    /// The signed-in account (`account/read`).
    pub async fn account(&self) -> Result<(bool, Option<CodexAccount>), ProviderError> {
        let v = self.conn().await?.request("account/read", json!({ "refreshToken": false })).await?;
        let a = account_of(&v);
        self.state.lock().account = Some(a.clone());
        Ok(a)
    }

    /// The last known account.
    #[must_use]
    pub fn cached_account(&self) -> Option<(bool, Option<CodexAccount>)> {
        self.state.lock().account.clone()
    }

    /// The sign-in in progress / last finished.
    #[must_use]
    pub fn login_state(&self) -> Option<CodexLoginState> {
        self.state.lock().login.clone()
    }

    /// Plan limits (`account/rateLimits/read`).
    pub async fn rate_limits(&self) -> Result<CodexLimits, ProviderError> {
        let v = self.conn().await?.request("account/rateLimits/read", Value::Null).await?;
        let l = map::limits(&v, Utc::now());
        self.state.lock().limits = Some(l.clone());
        Ok(l)
    }

    /// The last known limits.
    #[must_use]
    pub fn cached_limits(&self) -> Option<CodexLimits> {
        self.state.lock().limits.clone()
    }

    /// Models (`model/list`).
    pub async fn models(&self) -> Result<Vec<CodexModelOption>, ProviderError> {
        let v = self.conn().await?.request("model/list", json!({})).await?;
        Ok(map::models(&v))
    }

    /// Saved threads, newest first; only those under `cwd` when given.
    pub async fn threads(&self, cwd: Option<&Path>, limit: u32) -> Result<Vec<CodexThread>, ProviderError> {
        let want = cwd.map(|c| norm(&c.to_string_lossy()));
        let fetch = if want.is_some() { 200 } else { limit };
        let v = self.conn().await?.request("thread/list", json!({ "limit": fetch })).await?;
        let mut out: Vec<CodexThread> = v
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(map::thread)
            .filter(|t| match (&want, &t.cwd) {
                (Some(w), Some(c)) => norm(c).starts_with(w.as_str()),
                (Some(_), None) => false,
                (None, _) => true,
            })
            .collect();
        out.sort_by_key(|t| std::cmp::Reverse(t.updated_at));
        out.truncate(limit as usize);
        Ok(out)
    }

    /// A saved thread's items.
    pub async fn transcript(&self, thread_id: &str) -> Result<Vec<CodexEvent>, ProviderError> {
        if !super::is_token(thread_id) {
            return Err(err("invalid thread id"));
        }
        let v = self
            .conn()
            .await?
            .request_with("thread/read", json!({ "threadId": thread_id, "includeTurns": true }), THREAD_TIMEOUT)
            .await?;
        Ok(map::transcript(v.get("thread").unwrap_or(&Value::Null)))
    }

    /// Starts a session: a new thread, or `thread` resumed / forked.
    #[allow(clippy::too_many_arguments)]
    pub async fn start(
        &self,
        project_id: ProjectId,
        cwd: &Path,
        thread: Option<&str>,
        fork: bool,
        model: Option<String>,
        effort: Option<String>,
        mode: CodexApprovalMode,
    ) -> Result<CodexSessionInfo, ProviderError> {
        if let Some(t) = thread.filter(|t| !super::is_token(t)) {
            return Err(err(format!("invalid thread id `{t}`")));
        }
        let model = model.filter(|m| super::is_token(m) && m != "default");
        let effort = effort.filter(|e| super::is_token(e));
        let conn = self.conn().await?;
        let cwd_s = cwd.to_string_lossy().into_owned();
        let info = CodexSessionInfo {
            session_id: CodexSessionId::new(),
            project_id,
            cwd: cwd_s.clone(),
            thread_id: None,
            model: model.clone(),
            effort: effort.clone(),
            mode,
            state: CodexSessionState::Starting,
            title: None,
            started_at: Utc::now(),
            error: None,
        };
        let id = info.session_id;
        self.state
            .lock()
            .sessions
            .insert(id, Sess { info: info.clone(), mapper: PanelMapper::default(), pending: HashMap::new() });
        self.emit(HubEvent::Status(info));
        let mut p = super::thread_params(&cwd_s, model.as_deref(), mode.sandbox(), mode.approval_policy(), None, false);
        let method = match thread {
            Some(t) => {
                p["threadId"] = json!(t);
                p["excludeTurns"] = json!(true);
                if fork {
                    "thread/fork"
                } else {
                    "thread/resume"
                }
            }
            None => "thread/start",
        };
        let r = match conn.request_with(method, p, THREAD_TIMEOUT).await {
            Ok(r) => r,
            Err(e) => {
                let _ = self.update(id, |i| {
                    i.state = CodexSessionState::Failed;
                    i.error = Some(e.to_string());
                });
                self.state.lock().sessions.remove(&id);
                return Err(e);
            }
        };
        let thread_id = r.pointer("/thread/id").and_then(Value::as_str).unwrap_or_default().to_owned();
        let model = r.get("model").and_then(Value::as_str).map(str::to_owned).or(model);
        let effort = r.get("reasoningEffort").and_then(Value::as_str).map(str::to_owned).or(effort);
        let title = r
            .pointer("/thread/name")
            .and_then(Value::as_str)
            .or_else(|| r.pointer("/thread/preview").and_then(Value::as_str))
            .filter(|t| !t.trim().is_empty())
            .map(|t| t.chars().take(120).collect());
        let info = self.update(id, |i| {
            i.thread_id = Some(thread_id.clone());
            i.model.clone_from(&model);
            i.effort.clone_from(&effort);
            i.title = title;
            i.state = CodexSessionState::Idle;
        })?;
        self.item(&info, CodexEvent::ThreadStarted { thread_id, model, effort });
        Ok(info)
    }

    fn thread_of(&self, id: CodexSessionId) -> Result<(CodexSessionInfo, String), ProviderError> {
        let info = self.get(id)?;
        if info.state.is_final() {
            return Err(err("the Codex session has ended"));
        }
        let t = info.thread_id.clone().ok_or_else(|| err("the Codex session is still starting"))?;
        Ok((info, t))
    }

    /// Sends a user message (starts a turn).
    pub async fn send(&self, id: CodexSessionId, text: &str) -> Result<CodexSessionInfo, ProviderError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(err("empty message"));
        }
        let (info, thread) = self.thread_of(id)?;
        if matches!(info.state, CodexSessionState::Running | CodexSessionState::WaitingApproval) {
            return Err(err("a turn is already running — wait or interrupt it"));
        }
        let conn = self.conn().await?;
        let mut p = super::turn_params(&thread, text, info.effort.as_deref());
        p["approvalPolicy"] = json!(info.mode.approval_policy());
        p["sandboxPolicy"] = sandbox_policy(info.mode);
        if let Some(m) = &info.model {
            p["model"] = json!(m);
        }
        self.item(&info, CodexEvent::UserMessage { text: text.to_owned() });
        let info = self.update(id, |i| {
            i.state = CodexSessionState::Running;
            i.error = None;
            if i.title.is_none() {
                i.title = Some(text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(120).collect());
            }
        })?;
        if let Err(e) = conn.request("turn/start", p).await {
            self.item(&info, map::error_event(&e, false));
            return self.update(id, |i| {
                i.state = CodexSessionState::Idle;
                i.error = Some(e.to_string());
            });
        }
        Ok(info)
    }

    /// Interrupts the running turn.
    pub async fn interrupt(&self, id: CodexSessionId) -> Result<CodexSessionInfo, ProviderError> {
        let (info, thread) = self.thread_of(id)?;
        let turn = self.state.lock().sessions.get(&id).and_then(|s| s.mapper.turn_id.clone());
        if let Some(turn) = turn {
            self.conn().await?.request("turn/interrupt", json!({ "threadId": thread, "turnId": turn })).await?;
        }
        Ok(info)
    }

    /// Compacts the thread's context.
    pub async fn compact(&self, id: CodexSessionId) -> Result<CodexSessionInfo, ProviderError> {
        let (info, thread) = self.thread_of(id)?;
        self.conn().await?.request("thread/compact/start", json!({ "threadId": thread })).await?;
        Ok(info)
    }

    /// Model / effort / mode from the next turn.
    pub fn set_options(
        &self,
        id: CodexSessionId,
        model: Option<String>,
        effort: Option<String>,
        mode: Option<CodexApprovalMode>,
    ) -> Result<CodexSessionInfo, ProviderError> {
        if model.as_deref().is_some_and(|m| !super::is_token(m))
            || effort.as_deref().is_some_and(|e| !super::is_token(e))
        {
            return Err(err("invalid model or effort"));
        }
        let info = self.update(id, |i| {
            if let Some(m) = model {
                i.model = (m != "default").then_some(m);
            }
            if effort.is_some() {
                i.effort = effort;
            }
            if let Some(m) = mode {
                i.mode = m;
            }
        })?;
        self.item(&info, CodexEvent::ModelChanged { model: info.model.clone(), effort: info.effort.clone() });
        Ok(info)
    }

    /// Answers an approval request.
    pub async fn respond(
        &self,
        id: CodexSessionId,
        request_id: &str,
        decision: CodexApprovalDecision,
    ) -> Result<CodexSessionInfo, ProviderError> {
        let (pending, info) = {
            let mut st = self.state.lock();
            let s = st.sessions.get_mut(&id).ok_or_else(|| err("unknown Codex session"))?;
            let p = s.pending.remove(request_id).ok_or_else(|| err("no such approval request"))?;
            (p, s.info.clone())
        };
        // "Allow for the session" is never offered for a flagged request.
        let decision = if decision == CodexApprovalDecision::AllowSession && pending.risk.is_some() {
            CodexApprovalDecision::Allow
        } else {
            decision
        };
        let answer = match decision {
            CodexApprovalDecision::Allow => map::approval_answer(true, false, false),
            CodexApprovalDecision::AllowSession => map::approval_answer(true, true, false),
            CodexApprovalDecision::Deny => map::approval_answer(false, false, false),
            CodexApprovalDecision::Abort => map::approval_answer(false, false, true),
        };
        self.conn().await?.respond(&pending.rpc_id, answer);
        let ctx = CodexPolicyContext { session_id: id, project_id: info.project_id, cwd: PathBuf::from(&info.cwd) };
        let outcome = match decision {
            CodexApprovalDecision::Allow => PermissionOutcome::Allowed { session: false },
            CodexApprovalDecision::AllowSession => PermissionOutcome::Allowed { session: true },
            CodexApprovalDecision::Deny => PermissionOutcome::Denied,
            CodexApprovalDecision::Abort => PermissionOutcome::Cancelled,
        };
        self.policy.record(&ctx, &pending.tool, &pending.input, pending.risk.as_ref(), outcome).await;
        self.item(
            &info,
            CodexEvent::ApprovalResolved {
                request_id: request_id.to_owned(),
                decision,
                by_policy: false,
                message: None,
            },
        );
        let waiting = self.state.lock().sessions.get(&id).is_some_and(|s| !s.pending.is_empty());
        self.update(id, |i| {
            if i.state == CodexSessionState::WaitingApproval && !waiting {
                i.state = CodexSessionState::Running;
            }
        })
    }

    /// Ends a session.
    pub async fn stop(&self, id: CodexSessionId) -> Result<CodexSessionInfo, ProviderError> {
        let info = self.get(id)?;
        let pending: Vec<Pending> = self
            .state
            .lock()
            .sessions
            .get_mut(&id)
            .map(|s| s.pending.drain().map(|(_, p)| p).collect())
            .unwrap_or_default();
        if let Ok(conn) = self.conn().await {
            for p in pending {
                conn.respond(&p.rpc_id, map::approval_answer(false, false, true));
            }
            if let Some(t) = &info.thread_id {
                let turn = self.state.lock().sessions.get(&id).and_then(|s| s.mapper.turn_id.clone());
                if let Some(turn) = turn {
                    let _ = conn.request("turn/interrupt", json!({ "threadId": t, "turnId": turn })).await;
                }
                let _ = conn
                    .request_with("thread/unsubscribe", json!({ "threadId": t }), std::time::Duration::from_secs(10))
                    .await;
            }
        }
        let info = self.update(id, |i| i.state = CodexSessionState::Exited)?;
        self.item(&info, CodexEvent::Exited { code: None });
        self.state.lock().sessions.remove(&id);
        Ok(info)
    }

    /// Starts the ChatGPT sign-in.
    pub async fn login(&self, mode: CodexLoginMode) -> Result<CodexLoginState, ProviderError> {
        let conn = self.conn().await?;
        let params = match mode {
            CodexLoginMode::Browser => json!({ "type": "chatgpt" }),
            CodexLoginMode::DeviceCode => json!({ "type": "chatgptDeviceCode" }),
        };
        let r = conn.request("account/login/start", params).await?;
        let s = |k: &str| r.get(k).and_then(Value::as_str).map(str::to_owned);
        let done = self
            .login_state()
            .filter(|l| l.login_id.is_some() && l.login_id == s("loginId") && l.phase != CodexLoginPhase::Pending);
        let state = CodexLoginState {
            login_id: s("loginId"),
            mode,
            phase: done.as_ref().map_or(CodexLoginPhase::Pending, |d| d.phase),
            auth_url: s("authUrl"),
            verification_url: s("verificationUrl"),
            user_code: s("userCode"),
            error: done.and_then(|d| d.error),
            started_at: Utc::now(),
        };
        self.state.lock().login = Some(state.clone());
        self.emit(HubEvent::Account(CodexAccountPayload {
            logged_in: false,
            account: None,
            login: Some(state.clone()),
        }));
        Ok(state)
    }

    /// Cancels the sign-in in progress.
    pub async fn login_cancel(&self) -> Result<(), ProviderError> {
        let login = self.login_state();
        if let Some(id) =
            login.as_ref().filter(|l| l.phase == CodexLoginPhase::Pending).and_then(|l| l.login_id.clone())
        {
            let _ = self.conn().await?.request("account/login/cancel", json!({ "loginId": id })).await;
        }
        let mut st = self.state.lock();
        if let Some(l) = st.login.as_mut().filter(|l| l.phase == CodexLoginPhase::Pending) {
            l.phase = CodexLoginPhase::Cancelled;
        }
        Ok(())
    }

    /// Signs out.
    pub async fn logout(&self) -> Result<(), ProviderError> {
        self.conn().await?.request("account/logout", Value::Null).await?;
        self.state.lock().account = Some((false, None));
        self.emit(HubEvent::Account(CodexAccountPayload { logged_in: false, account: None, login: None }));
        Ok(())
    }
}

fn norm(p: &str) -> String {
    let s = p.replace('\\', "/").trim_end_matches('/').to_owned();
    if cfg!(windows) {
        s.to_lowercase()
    } else {
        s
    }
}

fn is_dangerous(r: Option<&ClaudeRisk>) -> bool {
    r.is_some()
}

/// Routes the app-server's notifications and requests to sessions.
async fn route(
    conn: std::sync::Weak<AppServer>,
    mut rx: mpsc::UnboundedReceiver<Inbound>,
    state: Arc<Mutex<State>>,
    events: mpsc::UnboundedSender<HubEvent>,
    policy: Arc<dyn CodexPolicy>,
) {
    let emit = |ev: HubEvent| {
        let _ = events.send(ev);
    };
    while let Some(msg) = rx.recv().await {
        match msg {
            Inbound::Closed => break,
            Inbound::Notification { method, params } => {
                match method.as_str() {
                    "account/login/completed" => {
                        let ok = params.get("success") == Some(&json!(true));
                        let login = {
                            let mut st = state.lock();
                            let id = params.get("loginId").and_then(Value::as_str).map(str::to_owned);
                            // The notification can overtake the `account/login/start` answer.
                            let l = st.login.get_or_insert_with(|| CodexLoginState {
                                login_id: id.clone(),
                                mode: CodexLoginMode::Browser,
                                phase: CodexLoginPhase::Pending,
                                auth_url: None,
                                verification_url: None,
                                user_code: None,
                                error: None,
                                started_at: Utc::now(),
                            });
                            if id.is_some() {
                                l.login_id = id;
                            }
                            l.phase = if ok { CodexLoginPhase::Succeeded } else { CodexLoginPhase::Failed };
                            l.error = params.get("error").and_then(Value::as_str).map(str::to_owned);
                            st.login.clone()
                        };
                        let account = match conn.upgrade() {
                            Some(c) if ok => c
                                .request("account/read", json!({ "refreshToken": false }))
                                .await
                                .ok()
                                .map(|v| account_of(&v)),
                            _ => None,
                        };
                        let (logged_in, account) = account.unwrap_or((ok, None));
                        state.lock().account = Some((logged_in, account.clone()));
                        emit(HubEvent::Account(CodexAccountPayload { logged_in, account, login }));
                        continue;
                    }
                    "account/updated" => {
                        if let Some(c) = conn.upgrade() {
                            if let Ok(v) = c.request("account/read", json!({ "refreshToken": false })).await {
                                let (logged_in, account) = account_of(&v);
                                state.lock().account = Some((logged_in, account.clone()));
                                let login = state.lock().login.clone();
                                emit(HubEvent::Account(CodexAccountPayload { logged_in, account, login }));
                            }
                        }
                        continue;
                    }
                    "account/rateLimits/updated" => {
                        let mut l = map::limits(params.get("rateLimits").unwrap_or(&Value::Null), Utc::now());
                        let merged = {
                            let mut st = state.lock();
                            if let Some(prev) = st.limits.as_ref() {
                                l.plan = l.plan.or_else(|| prev.plan.clone());
                                l.usage_allowed = l.usage_allowed.or(prev.usage_allowed);
                            }
                            if l.available {
                                st.limits = Some(l.clone());
                            }
                            l.available.then_some(l)
                        };
                        if let Some(l) = merged {
                            emit(HubEvent::Limits(l));
                        }
                        continue;
                    }
                    _ => {}
                }
                let Some(thread) = params.get("threadId").and_then(Value::as_str) else { continue };
                let mut out: Vec<HubEvent> = Vec::new();
                {
                    let mut st = state.lock();
                    let Some(s) = st.sessions.values_mut().find(|s| s.info.thread_id.as_deref() == Some(thread)) else {
                        continue;
                    };
                    let items = s.mapper.notification(&method, &params);
                    let before = s.info.state;
                    match method.as_str() {
                        "turn/started" => s.info.state = CodexSessionState::Running,
                        "turn/completed" => {
                            s.info.state = CodexSessionState::Idle;
                            s.pending.clear();
                        }
                        _ => {}
                    }
                    for e in &items {
                        if let CodexEvent::Error { message, will_retry: false, .. } = e {
                            s.info.error = Some(message.clone());
                        }
                    }
                    for event in items {
                        out.push(HubEvent::Item {
                            session_id: s.info.session_id,
                            project_id: s.info.project_id,
                            cwd: PathBuf::from(&s.info.cwd),
                            event,
                        });
                    }
                    if before != s.info.state {
                        out.push(HubEvent::Status(s.info.clone()));
                    }
                }
                for e in out {
                    emit(e);
                }
            }
            Inbound::Request { id, method, params } => {
                let Some(c) = conn.upgrade() else { break };
                let thread = params.get("threadId").and_then(Value::as_str).unwrap_or_default().to_owned();
                let found = {
                    let st = state.lock();
                    st.sessions.values().find(|s| s.info.thread_id.as_deref() == Some(thread.as_str())).map(|s| {
                        (s.info.clone(), map::approval(&method, &params, &s.mapper.file_items, &s.mapper.command_items))
                    })
                };
                let (info, Some(a)) = found.unwrap_or((dummy_info(), None)) else {
                    c.respond_error(&id, "not supported by Orkestr");
                    continue;
                };
                let ctx = CodexPolicyContext {
                    session_id: info.session_id,
                    project_id: info.project_id,
                    cwd: PathBuf::from(&info.cwd),
                };
                let risk = policy.assess(&ctx, &a.tool, &a.input);
                let request_id = match &id {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                let item = |event: CodexEvent| HubEvent::Item {
                    session_id: info.session_id,
                    project_id: info.project_id,
                    cwd: PathBuf::from(&info.cwd),
                    event,
                };
                if risk.as_ref().is_some_and(|r| r.level == ClaudeRiskLevel::Forbidden) {
                    c.respond(&id, map::approval_answer(false, false, false));
                    policy.record(&ctx, &a.tool, &a.input, risk.as_ref(), PermissionOutcome::AutoDenied).await;
                    emit(item(CodexEvent::ApprovalRequest {
                        request_id: request_id.clone(),
                        kind: a.kind,
                        tool_name: a.tool.clone(),
                        input: a.input.clone(),
                        reason: a.reason.clone(),
                        risk: risk.clone(),
                        can_remember: false,
                    }));
                    emit(item(CodexEvent::ApprovalResolved {
                        request_id,
                        decision: CodexApprovalDecision::Deny,
                        by_policy: true,
                        message: risk.map(|r| r.reason),
                    }));
                    continue;
                }
                let can_remember = !is_dangerous(risk.as_ref()) && a.kind == CodexApprovalKind::Command;
                let status = {
                    let mut st = state.lock();
                    st.sessions.get_mut(&info.session_id).map(|s| {
                        s.pending.insert(
                            request_id.clone(),
                            Pending {
                                rpc_id: id.clone(),
                                tool: a.tool.clone(),
                                input: a.input.clone(),
                                risk: risk.clone(),
                            },
                        );
                        s.info.state = CodexSessionState::WaitingApproval;
                        s.info.clone()
                    })
                };
                emit(item(CodexEvent::ApprovalRequest {
                    request_id,
                    kind: a.kind,
                    tool_name: a.tool,
                    input: a.input,
                    reason: a.reason,
                    risk,
                    can_remember,
                }));
                if let Some(s) = status {
                    emit(HubEvent::Status(s));
                }
            }
        }
    }
    // The process ended: every session ends with it.
    let ended: Vec<CodexSessionInfo> = {
        let mut st = state.lock();
        let v = st
            .sessions
            .values_mut()
            .map(|s| {
                s.info.state = CodexSessionState::Exited;
                s.info.clone()
            })
            .collect();
        st.sessions.clear();
        v
    };
    for info in ended {
        emit(HubEvent::Item {
            session_id: info.session_id,
            project_id: info.project_id,
            cwd: PathBuf::from(&info.cwd),
            event: CodexEvent::Exited { code: None },
        });
        emit(HubEvent::Status(info));
    }
}

fn dummy_info() -> CodexSessionInfo {
    CodexSessionInfo {
        session_id: CodexSessionId::new(),
        project_id: ProjectId::new(),
        cwd: String::new(),
        thread_id: None,
        model: None,
        effort: None,
        mode: CodexApprovalMode::Ask,
        state: CodexSessionState::Exited,
        title: None,
        started_at: Utc::now(),
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::super::conn::fake::FakeCodex;
    use super::*;
    use orkestr_domain::DangerCategory;

    /// Flags `rm -rf` as forbidden, `git push` as dangerous; records decisions.
    #[derive(Default)]
    struct TestPolicy {
        log: Mutex<Vec<(String, PermissionOutcome)>>,
    }

    #[async_trait]
    impl CodexPolicy for TestPolicy {
        fn assess(&self, _: &CodexPolicyContext, _: &str, input: &Value) -> Option<ClaudeRisk> {
            let c = input.get("command").and_then(Value::as_str).unwrap_or_default();
            let level = if c.contains("rm -rf") {
                ClaudeRiskLevel::Forbidden
            } else if c.contains("git push") {
                ClaudeRiskLevel::Dangerous
            } else {
                return None;
            };
            Some(ClaudeRisk { level, category: DangerCategory::RecursiveDelete, reason: "test".into() })
        }
        async fn record(
            &self,
            _: &CodexPolicyContext,
            _: &str,
            input: &Value,
            _: Option<&ClaudeRisk>,
            o: PermissionOutcome,
        ) {
            self.log.lock().push((input["command"].as_str().unwrap_or_default().to_owned(), o));
        }
    }

    fn script() -> super::super::conn::fake::Script {
        Arc::new(|method, p, id| match method {
            "account/read" => vec![
                json!({"id": id, "result": {"account": {"type":"chatgpt","email":"me@x.y","planType":"plus"}, "requiresOpenaiAuth": true}}),
            ],
            "account/rateLimits/read" => vec![
                json!({"id": id, "result": {"rateLimits": {"primary": {"usedPercent": 81, "windowDurationMins": 300}, "planType": "plus"}}}),
            ],
            "account/login/start" => vec![
                json!({"id": id, "result": {"type":"chatgpt","loginId":"L1","authUrl":"https://auth.openai.com/x"}}),
                json!({"method":"account/login/completed","params":{"loginId":"L1","success":true}}),
            ],
            "thread/list" => vec![json!({"id": id, "result": {"data": [
                {"id":"a","preview":"in project","cwd":"C:\\Proj\\x","updatedAt": 20},
                {"id":"b","preview":"elsewhere","cwd":"D:\\other","updatedAt": 30}
            ]}})],
            "thread/start" | "thread/fork" | "thread/resume" => vec![
                json!({"id": id, "result": {"thread": {"id": if method == "thread/fork" {"forked"} else {"th"}}, "model": "gpt-5-codex", "reasoningEffort": "medium"}}),
            ],
            "turn/start" => {
                let text = p.pointer("/input/0/text").and_then(Value::as_str).unwrap_or_default().to_owned();
                vec![
                    json!({"id": id, "result": {"turn": {"id": "u1", "items": [], "status": "inProgress"}}}),
                    json!({"method":"turn/started","params":{"threadId":"th","turn":{"id":"u1","items":[],"status":"inProgress"}}}),
                    json!({"id": format!("ap-{text}"), "method":"item/commandExecution/requestApproval","params":{"threadId":"th","turnId":"u1","itemId":"c1","command": text,"startedAtMs":1}}),
                ]
            }
            "response" => vec![
                json!({"method":"turn/completed","params":{"threadId":"th","turn":{"id":"u1","items":[],"status":"completed"}}}),
            ],
            _ if id >= 0 => vec![json!({"id": id, "result": {}})],
            _ => vec![],
        })
    }

    async fn hub() -> (CodexHub, mpsc::UnboundedReceiver<HubEvent>, Arc<FakeCodex>, Arc<TestPolicy>) {
        let fake = FakeCodex::new(script());
        let (tx, rx) = mpsc::unbounded_channel();
        let policy = Arc::new(TestPolicy::default());
        let hub =
            CodexHub::new(Arc::clone(&fake) as Arc<dyn ProcSpawner>, tx, Arc::clone(&policy) as Arc<dyn CodexPolicy>);
        hub.set_program(Some(PathBuf::from("codex")));
        (hub, rx, fake, policy)
    }

    async fn until(rx: &mut mpsc::UnboundedReceiver<HubEvent>, f: impl Fn(&HubEvent) -> bool) -> HubEvent {
        loop {
            let e = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv()).await.unwrap().unwrap();
            if f(&e) {
                return e;
            }
        }
    }

    #[tokio::test]
    async fn account_limits_login_and_threads() {
        let (hub, mut rx, fake, _) = hub().await;
        let (ok, acc) = hub.account().await.unwrap();
        assert!(ok);
        assert_eq!(acc.unwrap().plan.as_deref(), Some("plus"));
        let l = hub.rate_limits().await.unwrap();
        assert_eq!(l.primary.unwrap().percent, 81.0);
        let login = hub.login(CodexLoginMode::Browser).await.unwrap();
        assert_eq!(login.auth_url.as_deref(), Some("https://auth.openai.com/x"));
        let done = until(&mut rx, |e| {
            matches!(e, HubEvent::Account(p) if p.login.as_ref().is_some_and(|l| l.phase == CodexLoginPhase::Succeeded))
        })
        .await;
        assert!(matches!(done, HubEvent::Account(p) if p.logged_in));
        let threads = hub.threads(Some(Path::new("C:\\Proj")), 10).await.unwrap();
        if cfg!(windows) {
            assert_eq!(threads.len(), 1);
            assert_eq!(threads[0].id, "a");
        }
        assert_eq!(hub.threads(None, 10).await.unwrap()[0].id, "b", "newest first");
        assert_eq!(fake.spawned.load(std::sync::atomic::Ordering::SeqCst), 1, "one shared process");
    }

    #[tokio::test]
    async fn approvals_are_classified_answered_and_audited() {
        let (hub, mut rx, fake, policy) = hub().await;
        let s =
            hub.start(ProjectId::new(), Path::new("."), None, false, None, None, CodexApprovalMode::Ask).await.unwrap();
        assert_eq!(s.thread_id.as_deref(), Some("th"));
        assert_eq!(s.model.as_deref(), Some("gpt-5-codex"));

        // A dangerous command is asked (never remembered).
        hub.send(s.session_id, "git push").await.unwrap();
        let req =
            until(&mut rx, |e| matches!(e, HubEvent::Item { event: CodexEvent::ApprovalRequest { .. }, .. })).await;
        let HubEvent::Item { event: CodexEvent::ApprovalRequest { request_id, can_remember, risk, .. }, .. } = req
        else {
            unreachable!()
        };
        assert!(!can_remember && risk.is_some());
        assert_eq!(hub.get(s.session_id).unwrap().state, CodexSessionState::WaitingApproval);
        hub.respond(s.session_id, &request_id, CodexApprovalDecision::AllowSession).await.unwrap();
        until(&mut rx, |e| matches!(e, HubEvent::Item { event: CodexEvent::TurnCompleted { .. }, .. })).await;
        let answer = fake.seen.lock().iter().find(|v| v.get("result").is_some()).cloned().unwrap();
        assert_eq!(answer["result"]["decision"], "accept", "a flagged request is never allowed for the session");

        // A forbidden one is declined without asking.
        hub.send(s.session_id, "rm -rf /").await.unwrap();
        let res =
            until(&mut rx, |e| matches!(e, HubEvent::Item { event: CodexEvent::ApprovalResolved { .. }, .. })).await;
        assert!(matches!(
            res,
            HubEvent::Item {
                event: CodexEvent::ApprovalResolved { by_policy: true, decision: CodexApprovalDecision::Deny, .. },
                ..
            }
        ));
        let log = policy.log.lock().clone();
        assert_eq!(log[0], ("git push".into(), PermissionOutcome::Allowed { session: false }));
        assert_eq!(log[1], ("rm -rf /".into(), PermissionOutcome::AutoDenied));
        let turn = fake.seen.lock().iter().find(|v| v["method"] == "turn/start").cloned().unwrap();
        assert_eq!(turn["params"]["approvalPolicy"], "untrusted");
        assert_eq!(turn["params"]["sandboxPolicy"]["type"], "workspaceWrite");

        // Fork and stop.
        let f = hub
            .start(ProjectId::new(), Path::new("."), Some("th"), true, None, None, CodexApprovalMode::ReadOnly)
            .await
            .unwrap();
        assert_eq!(f.thread_id.as_deref(), Some("forked"));
        let st = hub.stop(f.session_id).await.unwrap();
        assert_eq!(st.state, CodexSessionState::Exited);
        assert!(hub.get(f.session_id).is_err());
        assert!(hub
            .start(ProjectId::new(), Path::new("."), Some("--bad"), false, None, None, CodexApprovalMode::Ask)
            .await
            .is_err());
    }
}
