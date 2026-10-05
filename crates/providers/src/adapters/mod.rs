//! Provider adapters. Provider-specific logic lives only here (§60.5).

pub mod anthropic;
pub mod claude_code;
pub mod cli;
pub mod codex;
pub mod gemini;
pub mod mock;
pub mod mock_review;
pub mod mock_merge;
pub mod mock_summarize;
pub mod mock_teams_pm;
pub mod ollama;
pub mod openai;
