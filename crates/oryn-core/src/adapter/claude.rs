//! Adapter for `claude --print --output-format stream-json --verbose`.
//!
//! Claude Code emits one JSON object per line. The shapes this adapter cares
//! about:
//!
//! ```text
//! {"type":"system","subtype":"init","session_id":"s1", ...}
//! {"type":"assistant","session_id":"s1","message":{"content":[{"type":"text","text":"..."}]}}
//! {"type":"assistant","session_id":"s1","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}}
//! {"type":"user","session_id":"s1","message":{"content":[{"type":"tool_result","tool_use_id":"t1","is_error":false,"content":"…"}]}}
//! {"type":"result","subtype":"success","session_id":"s1","total_cost_usd":0.012,"usage":{"input_tokens":1000,"output_tokens":200}}
//! ```
//!
//! Traversal is intentionally `serde_json::Value`-based rather than typed
//! structs: it tolerates unknown/added fields (format drift) without failing to
//! parse, which is a stated design goal — `raw` plus best-effort extraction
//! beats a brittle typed schema here.

use serde_json::Value;

use crate::adapter::AgentAdapter;
use crate::event::{AgentEvent, EventKind, TokenUsage};

/// Parses Claude Code's `stream-json` output. Stateless.
#[derive(Debug, Default, Clone, Copy)]
pub struct ClaudeAdapter;

const AGENT: &str = "claude";

impl ClaudeAdapter {
    fn base(session_id: &str, kind: EventKind, raw: &str) -> AgentEvent {
        AgentEvent::new(AGENT, session_id, kind, raw)
    }

    /// Extract a [`TokenUsage`] from a Claude `usage` object, mapping the cache
    /// fields. Absent fields default to zero.
    fn usage_from(usage: &Value) -> TokenUsage {
        let get = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
        TokenUsage {
            input: get("input_tokens"),
            output: get("output_tokens"),
            cache_read: get("cache_read_input_tokens"),
            cache_write: get("cache_creation_input_tokens"),
        }
    }

    /// Parse the content blocks of an `assistant` message into events.
    fn parse_assistant(session_id: &str, content: &[Value], raw: &str) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        for block in content {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => out.push(Self::base(session_id, EventKind::Message, raw)),
                Some("tool_use") => {
                    let mut ev = Self::base(session_id, EventKind::ToolUse, raw);
                    ev.tool = block.get("name").and_then(Value::as_str).map(String::from);
                    ev.tool_id = block.get("id").and_then(Value::as_str).map(String::from);
                    let input = block.get("input");
                    ev.command = input
                        .and_then(|i| i.get("command"))
                        .and_then(Value::as_str)
                        .map(String::from);
                    if let Some(fp) = input
                        .and_then(|i| i.get("file_path"))
                        .and_then(Value::as_str)
                    {
                        ev.files.push(fp.to_string());
                    }
                    out.push(ev);
                }
                _ => {}
            }
        }
        out
    }

    /// Parse the content blocks of a `user` message into `ToolResult` events.
    fn parse_user(session_id: &str, content: &[Value], raw: &str) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        for block in content {
            if block.get("type").and_then(Value::as_str) == Some("tool_result") {
                let mut ev = Self::base(session_id, EventKind::ToolResult, raw);
                ev.tool_id = block
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .map(String::from);
                ev.is_error = block
                    .get("is_error")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                out.push(ev);
            }
        }
        out
    }
}

impl AgentAdapter for ClaudeAdapter {
    fn name(&self) -> &str {
        AGENT
    }

    fn parse_line(&mut self, line: &str) -> Vec<AgentEvent> {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            return Vec::new();
        };
        let sid = v.get("session_id").and_then(Value::as_str).unwrap_or("");

        match v.get("type").and_then(Value::as_str) {
            Some("system") if v.get("subtype").and_then(Value::as_str) == Some("init") => {
                vec![Self::base(sid, EventKind::SessionStart, line)]
            }
            Some("assistant") => match v.pointer("/message/content").and_then(Value::as_array) {
                Some(content) => Self::parse_assistant(sid, content, line),
                None => Vec::new(),
            },
            Some("user") => match v.pointer("/message/content").and_then(Value::as_array) {
                Some(content) => Self::parse_user(sid, content, line),
                None => Vec::new(),
            },
            Some("result") => {
                let mut cost = Self::base(sid, EventKind::Cost, line);
                if let Some(usage) = v.get("usage") {
                    cost.usage = Some(Self::usage_from(usage));
                }
                cost.cost_usd = v.get("total_cost_usd").and_then(Value::as_f64);
                vec![cost, Self::base(sid, EventKind::SessionEnd, line)]
            }
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A complete, representative session as recorded NDJSON, including a
    /// tool_result and a result line with cost + cache usage.
    const FIXTURE: &str = concat!(
        r#"{"type":"system","subtype":"init","session_id":"s1","model":"claude-opus-4-8"}"#,
        "\n",
        r#"{"type":"assistant","session_id":"s1","message":{"content":[{"type":"text","text":"I'll list files."}]}}"#,
        "\n",
        r#"{"type":"assistant","session_id":"s1","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls -la"}}]}}"#,
        "\n",
        r#"{"type":"user","session_id":"s1","message":{"content":[{"type":"tool_result","tool_use_id":"t1","is_error":false,"content":"a.txt"}]}}"#,
        "\n",
        r#"{"type":"assistant","session_id":"s1","message":{"content":[{"type":"tool_use","id":"t2","name":"Edit","input":{"file_path":"src/main.rs"}}]}}"#,
        "\n",
        r#"{"type":"result","subtype":"success","session_id":"s1","total_cost_usd":0.012,"duration_ms":1234,"num_turns":3,"usage":{"input_tokens":1000,"output_tokens":200,"cache_read_input_tokens":50,"cache_creation_input_tokens":5}}"#,
    );

    fn adapter() -> ClaudeAdapter {
        ClaudeAdapter
    }

    #[test]
    fn name_is_claude() {
        assert_eq!(adapter().name(), "claude");
    }

    #[test]
    fn init_line_becomes_session_start() {
        let evs = adapter().parse_line(r#"{"type":"system","subtype":"init","session_id":"s1"}"#);
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].kind, EventKind::SessionStart);
        assert_eq!(evs[0].session_id, "s1");
        assert_eq!(evs[0].agent, "claude");
    }

    #[test]
    fn system_non_init_is_ignored() {
        let evs = adapter().parse_line(r#"{"type":"system","subtype":"other","session_id":"s1"}"#);
        assert!(evs.is_empty());
    }

    #[test]
    fn bash_tool_use_captures_command_and_id() {
        let line = r#"{"type":"assistant","session_id":"s1","message":{"content":[{"type":"tool_use","id":"toolu_9","name":"Bash","input":{"command":"ls -la"}}]}}"#;
        let evs = adapter().parse_line(line);
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].kind, EventKind::ToolUse);
        assert_eq!(evs[0].tool.as_deref(), Some("Bash"));
        assert_eq!(evs[0].tool_id.as_deref(), Some("toolu_9"));
        assert_eq!(evs[0].command.as_deref(), Some("ls -la"));
        assert!(evs[0].files.is_empty());
    }

    #[test]
    fn edit_tool_use_captures_file_path() {
        let line = r#"{"type":"assistant","session_id":"s1","message":{"content":[{"type":"tool_use","name":"Edit","input":{"file_path":"src/main.rs"}}]}}"#;
        let evs = adapter().parse_line(line);
        assert_eq!(evs[0].tool.as_deref(), Some("Edit"));
        assert_eq!(evs[0].files, vec!["src/main.rs".to_string()]);
        assert!(evs[0].command.is_none());
    }

    #[test]
    fn unrecognized_tool_still_emits_tool_use_event() {
        let line = r#"{"type":"assistant","session_id":"s1","message":{"content":[{"type":"tool_use","name":"MultiEdit"}]}}"#;
        let evs = adapter().parse_line(line);
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].kind, EventKind::ToolUse);
        assert_eq!(evs[0].tool.as_deref(), Some("MultiEdit"));
        assert!(evs[0].command.is_none());
        assert!(evs[0].files.is_empty());
    }

    #[test]
    fn non_string_command_is_ignored_not_coerced() {
        let line = r#"{"type":"assistant","session_id":"s1","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":42}}]}}"#;
        let evs = adapter().parse_line(line);
        assert_eq!(evs.len(), 1);
        assert!(evs[0].command.is_none());
    }

    #[test]
    fn tool_result_is_captured_with_pairing_id() {
        let line = r#"{"type":"user","session_id":"s1","message":{"content":[{"type":"tool_result","tool_use_id":"t1","is_error":false,"content":"ok"}]}}"#;
        let evs = adapter().parse_line(line);
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].kind, EventKind::ToolResult);
        assert_eq!(evs[0].tool_id.as_deref(), Some("t1"));
        assert!(!evs[0].is_error);
    }

    #[test]
    fn tool_result_error_flag_is_captured() {
        let line = r#"{"type":"user","session_id":"s1","message":{"content":[{"type":"tool_result","tool_use_id":"t1","is_error":true,"content":"boom"}]}}"#;
        let evs = adapter().parse_line(line);
        assert!(evs[0].is_error);
    }

    #[test]
    fn user_message_without_tool_result_is_ignored() {
        let line = r#"{"type":"user","session_id":"s1","message":{"content":[{"type":"text","text":"hi"}]}}"#;
        assert!(adapter().parse_line(line).is_empty());
    }

    #[test]
    fn multiple_content_blocks_yield_multiple_events() {
        let line = r#"{"type":"assistant","session_id":"s1","message":{"content":[{"type":"text","text":"hi"},{"type":"tool_use","name":"Bash","input":{"command":"pwd"}}]}}"#;
        let evs = adapter().parse_line(line);
        assert_eq!(evs.len(), 2);
        assert_eq!(evs[0].kind, EventKind::Message);
        assert_eq!(evs[1].kind, EventKind::ToolUse);
    }

    #[test]
    fn assistant_without_content_is_ignored() {
        let evs = adapter().parse_line(r#"{"type":"assistant","session_id":"s1","message":{}}"#);
        assert!(evs.is_empty());
    }

    #[test]
    fn user_without_content_is_ignored() {
        let evs = adapter().parse_line(r#"{"type":"user","session_id":"s1","message":{}}"#);
        assert!(evs.is_empty());
    }

    #[test]
    fn unknown_content_block_type_is_skipped() {
        let line = r#"{"type":"assistant","session_id":"s1","message":{"content":[{"type":"thinking","text":"…"}]}}"#;
        assert!(adapter().parse_line(line).is_empty());
    }

    #[test]
    fn result_line_emits_cost_then_session_end_with_usage_and_cost() {
        let line = r#"{"type":"result","session_id":"s1","total_cost_usd":0.0125,"usage":{"input_tokens":1000,"output_tokens":200,"cache_read_input_tokens":50,"cache_creation_input_tokens":5}}"#;
        let evs = adapter().parse_line(line);
        assert_eq!(evs.len(), 2);
        assert_eq!(evs[0].kind, EventKind::Cost);
        let usage = evs[0].usage.expect("usage present");
        assert_eq!(usage.input, 1000);
        assert_eq!(usage.output, 200);
        assert_eq!(usage.cache_read, 50);
        assert_eq!(usage.cache_write, 5);
        assert_eq!(usage.total(), 1255);
        assert_eq!(evs[0].cost_usd, Some(0.0125));
        assert_eq!(evs[1].kind, EventKind::SessionEnd);
    }

    #[test]
    fn error_result_still_emits_cost_and_end() {
        let line = r#"{"type":"result","subtype":"error","is_error":true,"session_id":"s1"}"#;
        let evs = adapter().parse_line(line);
        assert_eq!(evs.len(), 2);
        assert_eq!(evs[0].kind, EventKind::Cost);
        assert!(evs[0].usage.is_none());
        assert!(evs[0].cost_usd.is_none());
        assert_eq!(evs[1].kind, EventKind::SessionEnd);
    }

    #[test]
    fn junk_and_unknown_lines_are_ignored() {
        let mut a = adapter();
        assert!(a.parse_line("not json").is_empty());
        assert!(a.parse_line("").is_empty());
        assert!(a.parse_line(r#"{"type":"unknown"}"#).is_empty());
        assert!(a.parse_line(r#"{"no_type":true}"#).is_empty());
    }

    #[test]
    fn deeply_nested_json_does_not_panic() {
        let deep = format!("{}{}", "[".repeat(5000), "]".repeat(5000));
        assert!(adapter().parse_line(&deep).is_empty());
    }

    #[test]
    fn missing_session_id_defaults_to_empty() {
        let evs = adapter().parse_line(r#"{"type":"system","subtype":"init"}"#);
        assert_eq!(evs[0].session_id, "");
    }

    #[test]
    fn full_fixture_parses_expected_event_count() {
        let mut a = adapter();
        let total: usize = FIXTURE.lines().map(|l| a.parse_line(l).len()).sum();
        // 1 init + 1 text + 2 tool_use + 1 tool_result + (1 cost + 1 end) = 7
        assert_eq!(total, 7);
    }
}
