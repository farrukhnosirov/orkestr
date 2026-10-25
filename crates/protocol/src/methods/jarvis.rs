//! `jarvis.*` — the Jarvis persona and conversation mode (ADR-0049).
//!
//! - Persona settings (form of address, greeting on start, local templates,
//!   conversation model, memory length) are one JSON document in the core.
//! - `jarvis.match` answers small talk / FAQ from the local template bank
//!   (10 000+ Russian phrasings, no tokens spent); `matched: null` means the
//!   utterance is not small talk.
//! - `jarvis.chat` is the AI conversation: a Jarvis system prompt, the
//!   rolling context of the persisted history, the current date and time.
//!   Deltas stream as ephemeral `JarvisMessage` events (global topic); the
//!   result carries the whole reply.
//! - The whole area is local-only (it speaks for this machine's user).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{ModelRef, Timestamp};

use super::method;

/// How Jarvis addresses the user.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum JarvisAddress {
    /// «сэр» / "sir".
    #[default]
    Sir,
    /// «босс» / "boss".
    Boss,
    /// The user's name (`JarvisConfig::user_name`).
    Name,
    /// No form of address.
    None,
}

/// Jarvis persona and conversation settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
#[allow(clippy::struct_excessive_bools)]
pub struct JarvisConfig {
    /// Jarvis persona on (polite, calm, witty-dry). Off: neutral replies, no address.
    pub persona: bool,
    /// Form of address.
    pub address: JarvisAddress,
    /// The user's name (used when `address` is `Name`).
    pub user_name: String,
    /// Greet the user when the app starts / at the first wake of the day.
    pub greet_on_start: bool,
    /// Answer small talk from the local template bank (no tokens).
    pub local_templates: bool,
    /// Conversation model (`None`: the AI Manager's model, then routing).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub model: Option<ModelRef>,
    /// Messages of rolling context sent to the model (2..=60).
    pub memory_turns: u16,
    /// Give the model a short summary of the user's projects (names, running tasks).
    pub project_context: bool,
}

/// Max length of `JarvisConfig::user_name`.
pub const MAX_USER_NAME: usize = 40;
/// Messages kept in the persisted conversation history.
pub const HISTORY_CAP: usize = 200;
/// Max length of one chat message.
pub const MAX_CHAT_CHARS: usize = 4_000;

impl Default for JarvisConfig {
    fn default() -> Self {
        Self {
            persona: true,
            address: JarvisAddress::Sir,
            user_name: String::new(),
            greet_on_start: true,
            local_templates: true,
            model: None,
            memory_turns: 12,
            project_context: true,
        }
    }
}

impl JarvisConfig {
    /// Validates field ranges.
    ///
    /// # Errors
    /// A human-readable message for the first invalid field.
    pub fn validate(&self) -> Result<(), String> {
        if self.user_name.chars().count() > MAX_USER_NAME || self.user_name.chars().any(char::is_control) {
            return Err(format!("userName must be at most {MAX_USER_NAME} printable characters"));
        }
        if !(2..=60).contains(&self.memory_turns) {
            return Err("memoryTurns must be 2..=60".into());
        }
        if self.address == JarvisAddress::Name && self.user_name.trim().is_empty() {
            return Err("address Name needs a userName".into());
        }
        Ok(())
    }
}

/// Result of `jarvis.getConfig` / `jarvis.setConfig`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct JarvisConfigResult {
    /// Settings.
    pub config: JarvisConfig,
    /// Template bank size.
    pub bank: JarvisBankStats,
}

/// Size of the local template bank.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct JarvisBankStats {
    /// Intents.
    pub intents: u32,
    /// Question phrasings (patterns).
    pub questions: u32,
    /// Answer variants.
    pub answers: u32,
}

/// Params of `jarvis.setConfig`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SetJarvisConfigParams {
    /// New settings.
    pub config: JarvisConfig,
}

/// Params of `jarvis.match`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct JarvisMatchParams {
    /// Utterance.
    pub text: String,
    /// UI language (`ru`, `en`). The bank is Russian; other languages never match.
    pub lang: String,
    /// The last thing Jarvis said (for «повтори»).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub last_reply: Option<String>,
}

/// A template answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct JarvisTemplateReply {
    /// Intent id (`greet.hello`, `time.now`, `mode.chat`, …).
    pub intent: String,
    /// Category (`greetings`, `time`, …).
    pub category: String,
    /// Client action of the intent (`mode.chat`, `mode.commands`,
    /// `memory.forget`, `speech.stop`, `repeat`, `address.sir|boss|none`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub action: Option<String>,
    /// Rendered answer (address, time, date filled in).
    pub text: String,
    /// Match score 0..1.
    pub score: f32,
    /// Phrase that matched.
    pub phrase: String,
}

/// Result of `jarvis.match`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct JarvisMatchResult {
    /// The answer, or `None` (not small talk / templates off).
    pub matched: Option<JarvisTemplateReply>,
    /// Matching time, µs.
    pub elapsed_us: u64,
}

/// Who said a history message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum JarvisRole {
    /// The user.
    User,
    /// Jarvis.
    Assistant,
}

/// One message of the conversation history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct JarvisMessage {
    /// Id.
    pub id: String,
    /// Speaker.
    pub role: JarvisRole,
    /// Text.
    pub text: String,
    /// When.
    pub at: Timestamp,
    /// How the reply was produced: `ai`, `template`, `command`, `user`.
    pub source: String,
}

/// Params of `jarvis.chat`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct JarvisChatParams {
    /// User message.
    pub text: String,
    /// Reply language.
    pub lang: String,
    /// Client-chosen id of the reply (stream correlation; generated when absent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub message_id: Option<String>,
    /// Spoken reply: shorter, no markdown.
    #[serde(default)]
    pub spoken: bool,
}

/// Result of `jarvis.chat`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct JarvisChatResult {
    /// Reply id.
    pub message_id: String,
    /// Reply text (without the command hand-off line).
    pub text: String,
    /// Model that answered.
    pub model: String,
    /// A computer command the model handed off (`<<run: …>>`), for the client to run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub command: Option<String>,
}

/// Params of `jarvis.cancel`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct JarvisCancelParams {
    /// Reply id.
    pub message_id: String,
}

/// Params of `jarvis.history`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct JarvisHistoryParams {
    /// Newest messages to return (default 50).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub limit: Option<u32>,
}

/// Result of `jarvis.history`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct JarvisHistoryResult {
    /// Oldest first.
    pub messages: Vec<JarvisMessage>,
}

/// Params of `jarvis.record`: an exchange answered outside `jarvis.chat`
/// (template answer, executed command) joins the conversation memory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct JarvisRecordParams {
    /// What the user said.
    pub user: String,
    /// What Jarvis answered.
    pub reply: String,
    /// `template` or `command`.
    pub source: String,
}

/// `JarvisMessage` event payload: a streamed delta of a `jarvis.chat` reply
/// (ephemeral, global topic).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct JarvisDeltaPayload {
    /// Reply id.
    pub message_id: String,
    /// Text delta (append to the previous ones).
    pub delta: String,
    /// The provider switched mid-answer (fallback): drop the text so far.
    #[serde(default)]
    pub reset: bool,
    /// True on the last delta.
    pub done: bool,
}

method!(
    /// `jarvis.getConfig`.
    GetConfig, "jarvis.getConfig", super::Empty, JarvisConfigResult
);
method!(
    /// `jarvis.setConfig`.
    SetConfig, "jarvis.setConfig", SetJarvisConfigParams, JarvisConfigResult
);
method!(
    /// `jarvis.match`: small talk / FAQ from the local template bank.
    Match, "jarvis.match", JarvisMatchParams, JarvisMatchResult
);
method!(
    /// `jarvis.chat`: AI conversation turn (streams `JarvisMessage` events).
    Chat, "jarvis.chat", JarvisChatParams, JarvisChatResult
);
method!(
    /// `jarvis.cancel`: stops a streaming reply.
    Cancel, "jarvis.cancel", JarvisCancelParams, super::Empty
);
method!(
    /// `jarvis.history`: the persisted conversation.
    History, "jarvis.history", JarvisHistoryParams, JarvisHistoryResult
);
method!(
    /// `jarvis.record`: adds a template / command exchange to the conversation memory.
    Record, "jarvis.record", JarvisRecordParams, super::Empty
);
method!(
    /// `jarvis.forget`: clears the conversation («забудь разговор»).
    Forget, "jarvis.forget", super::Empty, super::Empty
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_validate_and_roundtrip() {
        let c = JarvisConfig::default();
        assert!(c.validate().is_ok());
        assert_eq!(c.address, JarvisAddress::Sir);
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["address"], "Sir");
        assert_eq!(v["memoryTurns"], 12);
        let back: JarvisConfig = serde_json::from_value(serde_json::json!({"address": "Boss"})).unwrap();
        assert_eq!(back.address, JarvisAddress::Boss);
        assert!(back.persona, "missing fields default");
    }

    #[test]
    fn validation() {
        let c = JarvisConfig { address: JarvisAddress::Name, ..JarvisConfig::default() };
        assert!(c.validate().is_err());
        let c = JarvisConfig { memory_turns: 1, ..JarvisConfig::default() };
        assert!(c.validate().is_err());
        let c =
            JarvisConfig { address: JarvisAddress::Name, user_name: "Фаррух".into(), ..JarvisConfig::default() };
        assert!(c.validate().is_ok());
    }
}
