//! Read-only queries of the CLI (no model tokens): plan limits (`get_usage`,
//! what `/usage` shows), the context window (`get_context_usage`, `/context`)
//! and MCP servers (`mcp_status`, `/mcp`).
//!
//! Recorded against Claude Code 2.1.292 (`tests/fixtures/claude_code/queries.jsonl`):
//!
//! * `get_usage` → `{subscription_type, rate_limits_available, rate_limits:
//!   {five_hour: {utilization: 11, resets_at: "2026-…+00:00"}, seven_day: {…},
//!   seven_day_opus | seven_day_sonnet: {…} | null, extra_usage: {is_enabled},
//!   limits: [{kind: session | weekly_all | weekly_scoped, percent, severity,
//!   resets_at, scope: {model: {display_name}}}]}}`. `utilization` is a
//!   percentage (0‥100).
//! * `get_context_usage` → `{totalTokens, maxTokens, percentage, categories:
//!   [{name, tokens, kind: used | deferred | free}]}`.
//! * `mcp_status` → `{mcpServers: [{name, status, error?, scope, tools?: […]}]}`.
//!
//! Without a live session, [`probe_usage`] starts a short-lived process,
//! sends `initialize` + `get_usage` and kills it.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use chrono::{DateTime, Utc};
use orkestr_domain::now;
use orkestr_protocol::methods::claude_code::{
    ClaudeContextCategory, ClaudeContextUsage, ClaudeLimitWindow, ClaudeMcpServer, ClaudePlanUsage,
};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use super::control;

/// How long a usage probe may take.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(40);

fn ts(v: &Value) -> Option<DateTime<Utc>> {
    let s = v.as_str()?;
    DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&Utc))
}

fn num(v: &Value) -> Option<f64> {
    v.as_f64().filter(|n| n.is_finite())
}

fn window(v: &Value, label: Option<String>, severity: Option<String>) -> Option<ClaudeLimitWindow> {
    let percent = num(v.get("utilization")?).or_else(|| v.get("percent").and_then(num))?;
    Some(ClaudeLimitWindow {
        percent: percent.clamp(0.0, 100.0),
        resets_at: v.get("resets_at").and_then(ts),
        label,
        severity,
    })
}

fn severity_of(limits: &[Value], kind: &str) -> Option<String> {
    limits
        .iter()
        .find(|l| l.get("kind").and_then(Value::as_str) == Some(kind))
        .and_then(|l| l.get("severity").and_then(Value::as_str))
        .map(str::to_owned)
}

/// Plan limits from a `get_usage` answer.
#[must_use]
pub fn plan_usage(resp: &Value) -> ClaudePlanUsage {
    let rl = resp.get("rate_limits").cloned().unwrap_or(Value::Null);
    let limits: Vec<Value> =
        rl.get("limits").or_else(|| resp.get("limits")).and_then(Value::as_array).cloned().unwrap_or_default();
    let session = rl.get("five_hour").and_then(|w| window(w, None, severity_of(&limits, "session")));
    let weekly = rl.get("seven_day").and_then(|w| window(w, None, severity_of(&limits, "weekly_all")));
    let mut weekly_models: Vec<ClaudeLimitWindow> = Vec::new();
    for (key, label) in [("seven_day_opus", "Opus"), ("seven_day_sonnet", "Sonnet")] {
        if let Some(w) = rl.get(key).and_then(|w| window(w, Some(label.to_owned()), None)) {
            weekly_models.push(w);
        }
    }
    for l in &limits {
        if l.get("kind").and_then(Value::as_str) != Some("weekly_scoped") {
            continue;
        }
        let Some(label) = l.pointer("/scope/model/display_name").and_then(Value::as_str) else { continue };
        if weekly_models.iter().any(|w| w.label.as_deref() == Some(label)) {
            continue;
        }
        if let Some(percent) = l.get("percent").and_then(num) {
            weekly_models.push(ClaudeLimitWindow {
                percent: percent.clamp(0.0, 100.0),
                resets_at: l.get("resets_at").and_then(ts),
                label: Some(label.to_owned()),
                severity: l.get("severity").and_then(Value::as_str).map(str::to_owned),
            });
        }
    }
    let available = resp.get("rate_limits_available").and_then(Value::as_bool).unwrap_or(false)
        && (session.is_some() || weekly.is_some());
    ClaudePlanUsage {
        available,
        subscription_type: resp.get("subscription_type").and_then(Value::as_str).map(str::to_owned),
        session,
        weekly,
        weekly_models,
        extra_usage_enabled: rl.pointer("/extra_usage/is_enabled").and_then(Value::as_bool),
        fetched_at: now(),
        error: if available { None } else { Some("Claude Code reported no plan limits".into()) },
    }
}

/// An empty answer with an error (no data).
#[must_use]
pub fn unavailable(error: impl Into<String>) -> ClaudePlanUsage {
    ClaudePlanUsage {
        available: false,
        subscription_type: None,
        session: None,
        weekly: None,
        weekly_models: Vec::new(),
        extra_usage_enabled: None,
        fetched_at: now(),
        error: Some(error.into()),
    }
}

/// Context window usage from a `get_context_usage` answer.
#[must_use]
pub fn context_usage(resp: &Value) -> ClaudeContextUsage {
    let u = |k: &str| resp.get(k).and_then(Value::as_u64).unwrap_or(0);
    let categories = resp
        .get("categories")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|c| {
            Some(ClaudeContextCategory {
                name: c.get("name")?.as_str()?.to_owned(),
                tokens: c.get("tokens").and_then(Value::as_u64).unwrap_or(0),
                kind: c.get("kind").and_then(Value::as_str).unwrap_or("used").to_owned(),
            })
        })
        .collect();
    let (total, max) = (u("totalTokens"), u("maxTokens"));
    #[allow(clippy::cast_precision_loss)]
    let percentage = resp.get("percentage").and_then(num).unwrap_or_else(|| {
        if max > 0 {
            total as f64 * 100.0 / max as f64
        } else {
            0.0
        }
    });
    ClaudeContextUsage { total_tokens: total, max_tokens: max, percentage, categories }
}

/// MCP servers from an `mcp_status` answer (configs and secrets are dropped).
#[must_use]
pub fn mcp_servers(resp: &Value) -> Vec<ClaudeMcpServer> {
    resp.get("mcpServers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|m| {
            Some(ClaudeMcpServer {
                name: m.get("name")?.as_str()?.to_owned(),
                status: m.get("status").and_then(Value::as_str).unwrap_or("unknown").to_owned(),
                error: m.get("error").and_then(Value::as_str).map(|e| e.chars().take(300).collect()),
                scope: m.get("scope").and_then(Value::as_str).map(str::to_owned),
                tools: m
                    .get("tools")
                    .and_then(Value::as_array)
                    .map_or(0, |t| u32::try_from(t.len()).unwrap_or(u32::MAX)),
            })
        })
        .collect()
}

/// Asks a short-lived CLI process for the plan limits (`initialize` +
/// `get_usage`; no conversation, no model tokens) and kills it.
pub async fn probe_usage(program: &Path, cwd: &Path) -> Result<ClaudePlanUsage, String> {
    let args: Vec<String> = [
        "--print",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-mode",
        "plan",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    let mut cmd = super::manager::sanitized_command(program, &args);
    cmd.current_dir(cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| format!("could not run `{}`: {e}", program.display()))?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let run = async {
        for (id, req) in [("orkestr-init", control::initialize()), ("orkestr-usage", control::query("get_usage"))] {
            let line = control::control_request(id, req) + "\n";
            stdin.write_all(line.as_bytes()).await.map_err(|e| e.to_string())?;
        }
        stdin.flush().await.map_err(|e| e.to_string())?;
        let mut lines = BufReader::new(stdout).lines();
        while let Some(line) = lines.next_line().await.map_err(|e| e.to_string())? {
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            if v.get("type").and_then(Value::as_str) != Some("control_response") {
                continue;
            }
            let r = v.get("response").cloned().unwrap_or(Value::Null);
            if r.get("request_id").and_then(Value::as_str) != Some("orkestr-usage") {
                continue;
            }
            if r.get("subtype").and_then(Value::as_str) == Some("error") {
                return Err(r.get("error").and_then(Value::as_str).unwrap_or("error").to_owned());
            }
            return Ok(plan_usage(r.get("response").unwrap_or(&Value::Null)));
        }
        Err("Claude Code exited without answering".to_owned())
    };
    let out =
        tokio::time::timeout(PROBE_TIMEOUT, run).await.unwrap_or_else(|_| Err("Claude Code did not answer".into()));
    let _ = child.start_kill();
    let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const QUERIES: &str = include_str!("../../tests/fixtures/claude_code/queries.jsonl");

    fn answer(id: &str) -> Value {
        QUERIES
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .find(|v| v.pointer("/response/request_id").and_then(Value::as_str) == Some(id))
            .and_then(|v| v.pointer("/response/response").cloned())
            .expect("recorded answer")
    }

    #[test]
    fn plan_limits_from_recorded_get_usage() {
        let u = plan_usage(&answer("r0"));
        assert!(u.available);
        assert_eq!(u.subscription_type.as_deref(), Some("max"));
        let s = u.session.expect("session window");
        assert!((s.percent - 14.0).abs() < f64::EPSILON);
        assert_eq!(s.resets_at.unwrap().to_rfc3339(), "2026-10-08T10:40:00.284486+00:00");
        assert_eq!(s.severity.as_deref(), Some("normal"));
        let w = u.weekly.expect("weekly window");
        assert!((w.percent - 23.0).abs() < f64::EPSILON);
        assert_eq!(u.weekly_models.len(), 1);
        assert_eq!(u.weekly_models[0].label.as_deref(), Some("Fable"));
        assert_eq!(u.extra_usage_enabled, Some(false));
        assert!(u.error.is_none());
    }

    #[test]
    fn missing_limits_are_unavailable() {
        let u = plan_usage(&serde_json::json!({"subscription_type": null, "rate_limits_available": false}));
        assert!(!u.available && u.session.is_none() && u.error.is_some());
        let u = plan_usage(&serde_json::json!({
            "rate_limits_available": true,
            "rate_limits": {"five_hour": {"utilization": 140, "resets_at": "bad"}, "seven_day_opus": {"utilization": 3}}
        }));
        assert!(u.available);
        assert!((u.session.as_ref().unwrap().percent - 100.0).abs() < f64::EPSILON, "clamped");
        assert!(u.session.unwrap().resets_at.is_none());
        assert_eq!(u.weekly_models[0].label.as_deref(), Some("Opus"));
    }

    #[test]
    fn context_and_mcp_from_recorded_answers() {
        let c = context_usage(&answer("r1"));
        assert_eq!(c.max_tokens, 200_000);
        assert!(c.total_tokens > 0 && c.percentage > 0.0);
        assert!(c.categories.iter().any(|k| k.kind == "free"));
        assert!(c.categories.iter().any(|k| k.name == "Messages"));
        let m = mcp_servers(&answer("r2"));
        assert!(!m.is_empty());
        assert!(m.iter().any(|s| s.status == "needs-auth"));
        assert!(m.iter().any(|s| s.status == "connected" && s.tools > 0));
    }
}
