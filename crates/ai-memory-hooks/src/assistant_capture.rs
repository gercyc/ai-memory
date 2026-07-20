//! Assistant-message capture plumbing (issue #196).
//!
//! Some agent harnesses attach the assistant's final turn to their `Stop`
//! lifecycle event — Claude Code sends it as a top-level `last_assistant_message`
//! string. That text is high-value for recall but privacy-sensitive: it can
//! quote code, secrets, or content from paths ai-memory never sees. Capturing
//! it is therefore an explicit, opt-in feature landing across later PRs.
//!
//! This module owns the single source of truth for WHICH raw field carries the
//! assistant message per agent/event, plus the unconditional strip that keeps
//! that raw field off the local spool, the wire, tracing, and storage until the
//! opt-in path deliberately re-introduces a sanitized, capped excerpt.

use ai_memory_core::AgentKind;

use crate::payload::HookEvent;

/// Protocol version for the opt-in `_ai_memory_assistant` body marker the client
/// attaches once capture is enabled (later PR). Defined here so the wire
/// contract has one home from the start.
pub const ASSISTANT_PROTOCOL_VERSION: u8 = 1;

/// Hard ceiling on the raw assistant-message string the client reads before
/// sanitizing/truncating (later PR). Oversized input is treated as absent.
pub const ASSISTANT_MESSAGE_MAX_INPUT_BYTES: usize = 64 * 1024;

/// Byte cap on the sanitized excerpt the opt-in path persists (later PR). Kept
/// equal to `truncate_excerpt`'s existing 2 KB excerpt contract so Stop bodies
/// do not become a second, larger excerpt norm.
pub const ASSISTANT_EXCERPT_MAX_BYTES: usize = 2_000;

/// Every raw top-level field name known to carry an assistant message across
/// supported agents. The unconditional strip removes each of these; the closed
/// per-agent table below decides which is a *candidate* for opt-in capture. The
/// union is kept tiny on purpose — one entry per distinct wire spelling.
const ASSISTANT_MESSAGE_FIELDS: &[&str] = &["last_assistant_message"];

/// The raw field that carries the assistant's final message for `(agent, event)`,
/// or `None` when the pair has no verified assistant-message field.
///
/// Closed table: only `ClaudeCode + Stop` is supported today. Extend
/// deliberately — a new entry opts an agent/event into capture and MUST have its
/// field name present in [`ASSISTANT_MESSAGE_FIELDS`] so the strip covers it
/// (enforced by `closed_table_fields_are_all_stripped`).
#[must_use]
pub fn assistant_message_field(agent: AgentKind, event: HookEvent) -> Option<&'static str> {
    match (agent, event) {
        (AgentKind::ClaudeCode, HookEvent::Stop) => Some("last_assistant_message"),
        _ => None,
    }
}

/// Unconditionally remove every known assistant-message field from a raw hook
/// payload's top-level object, returning whether anything was removed.
///
/// This is a defense applied on BOTH sides of the wire (client pre-spool and
/// server pre-envelope) and for EVERY agent/event, not just the supported pair:
/// a raw assistant-message field must never reach the spool, the wire, tracing,
/// or storage unless the explicit opt-in path (later PR) re-introduces it as a
/// sanitized, capped excerpt. Only top-level keys are inspected — the same scope
/// as `body_is_subagent`, and where every supported harness places the field.
pub fn strip_assistant_message_raw(raw: &mut serde_json::Value) -> bool {
    let Some(object) = raw.as_object_mut() else {
        return false;
    };
    let mut removed = false;
    for field in ASSISTANT_MESSAGE_FIELDS {
        if object.remove(*field).is_some() {
            removed = true;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only `ClaudeCode + Stop` is a capture candidate; every other agent/event
    /// pair across the full agent surface must return `None`.
    #[test]
    fn only_claude_stop_is_a_capture_candidate() {
        let events = [
            HookEvent::SessionStart,
            HookEvent::UserPrompt,
            HookEvent::PreToolUse,
            HookEvent::PostToolUse,
            HookEvent::PreCompact,
            HookEvent::PostCompaction,
            HookEvent::Notification,
            HookEvent::Stop,
            HookEvent::SessionEnd,
            HookEvent::SubagentStart,
            HookEvent::SubagentStop,
            HookEvent::Other,
        ];
        for agent in AgentKind::ALL {
            for event in events {
                let expected = agent == AgentKind::ClaudeCode && event == HookEvent::Stop;
                assert_eq!(
                    assistant_message_field(agent, event).is_some(),
                    expected,
                    "agent={agent:?} event={event:?}"
                );
            }
        }
    }

    /// The strip must cover every field the closed table can name, or an opted-in
    /// agent/event could carry a raw field the strip misses.
    #[test]
    fn closed_table_fields_are_all_stripped() {
        let events = [
            HookEvent::SessionStart,
            HookEvent::UserPrompt,
            HookEvent::PreToolUse,
            HookEvent::PostToolUse,
            HookEvent::PreCompact,
            HookEvent::PostCompaction,
            HookEvent::Notification,
            HookEvent::Stop,
            HookEvent::SessionEnd,
            HookEvent::SubagentStart,
            HookEvent::SubagentStop,
            HookEvent::Other,
        ];
        for agent in AgentKind::ALL {
            for event in events {
                if let Some(field) = assistant_message_field(agent, event) {
                    assert!(
                        ASSISTANT_MESSAGE_FIELDS.contains(&field),
                        "closed-table field {field:?} is not in the strip set"
                    );
                }
            }
        }
    }

    /// The strip is unconditional: it removes the raw field regardless of agent
    /// and reports the removal, so a client that never verified the agent still
    /// cannot leak it.
    #[test]
    fn strip_removes_field_and_reports_change() {
        let mut raw = serde_json::json!({
            "session_id": "s1",
            "last_assistant_message": "SENTINEL_ASSISTANT_MESSAGE",
        });
        assert!(strip_assistant_message_raw(&mut raw));
        assert!(raw.get("last_assistant_message").is_none());
        assert_eq!(raw.get("session_id").and_then(|v| v.as_str()), Some("s1"));
    }

    #[test]
    fn strip_is_noop_when_absent_or_not_object() {
        let mut without = serde_json::json!({"session_id": "s1"});
        assert!(!strip_assistant_message_raw(&mut without));
        assert_eq!(without, serde_json::json!({"session_id": "s1"}));

        let mut array = serde_json::json!(["last_assistant_message"]);
        assert!(!strip_assistant_message_raw(&mut array));
        assert_eq!(array, serde_json::json!(["last_assistant_message"]));
    }
}
