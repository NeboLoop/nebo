//! An Anthropic model reached through Janus's OpenAI-format door (Nebo AI's
//! Claude models and the owner's own Anthropic key alike): the request
//! extensions Janus maps onto Anthropic's Messages API, in OpenRouter's
//! conventions, so caching, thinking and deferred tools work as they did on
//! a direct client.
//!
//! - Each stable system chunk (`ChatRequest::cache_breakpoints`) is its own
//!   leading system message whose text part carries
//!   `cache_control: {type: ephemeral}`; the last tool loaded up front and
//!   the last message's last part carry one too (at most four markers).
//! - A deferred tool a result loaded is sent with `defer_loading`, and the
//!   result names it with a `{type: tool_reference, tool_name}` part in place
//!   of its definition's text: a load appends to the conversation and the
//!   cached prefix before it stays whole. (Janus loads by reference on Claude
//!   4.5 and later and falls back to a line naming the tool on older ones.)
//! - A marker on a tool result's part goes on that `tool_result` block.
//! - Thinking asks `reasoning: {max_tokens}` (the pack level's budget, 10,000
//!   when none is set), and an assistant turn's signed thinking blocks go back
//!   on it as `reasoning_details`, so thinking runs inside tool loops: every
//!   item in the order and with the content it came in, in Claude's format,
//!   the reply's block order (`reasoning.order`) included, so Janus replays
//!   the turn exactly as Claude wrote it. Items in another format are dropped.

use std::collections::{HashMap, HashSet};

use serde_json::{Value, json};

use crate::types::{ChatRequest, ThinkingBlock, definition_of, functions_block};

/// The cache markers one request may carry, counted across tools, system
/// and messages (Anthropic's limit).
const MAX_CACHE_MARKERS: usize = 4;

/// The `format` of Claude's own reasoning items: the only reasoning Nebo
/// reads back from Janus and sends to it.
const CLAUDE_FORMAT: &str = "anthropic-claude-v1";

/// Whether a request to `model` goes to an Anthropic model: the owner's own
/// Anthropic key, or a Claude model of Nebo AI's.
pub(super) fn is_anthropic(own_key_provider: Option<&str>, provider_id: &str, model: &str) -> bool {
    match own_key_provider {
        Some(p) => p == "anthropic",
        None => provider_id == "janus" && model.starts_with("claude"),
    }
}

/// The thinking budget a pack level's provider effort asks for; 10,000
/// tokens when none is set.
pub(super) fn thinking_budget(effort: Option<types::packs::ProviderEffort>) -> i32 {
    use types::packs::ProviderEffort::*;
    match effort {
        Some(Low) => 4_000,
        Some(Medium) | None => 10_000,
        Some(High) => 24_000,
    }
}

/// A five-minute cache breakpoint: Janus prices five-minute writes only and
/// refuses a `ttl` of `1h`, so none is named.
fn marker() -> Value {
    json!({"type": "ephemeral"})
}

/// The signed thinking blocks of an assistant turn, as `reasoning_details`,
/// in the order and with the content they came in, each in Claude's format.
pub(super) fn reasoning_details(blocks: &[ThinkingBlock]) -> Value {
    Value::Array(
        blocks
            .iter()
            .map(|b| match b {
                ThinkingBlock::Thinking {
                    thinking,
                    signature,
                } => {
                    json!({"type": "reasoning.text", "text": thinking, "signature": signature, "format": CLAUDE_FORMAT})
                }
                ThinkingBlock::RedactedThinking { data } => {
                    json!({"type": "reasoning.encrypted", "data": data, "format": CLAUDE_FORMAT})
                }
                ThinkingBlock::BlockOrder { blocks } => {
                    json!({"type": "reasoning.order", "format": CLAUDE_FORMAT, "blocks": blocks})
                }
            })
            .collect(),
    )
}

/// Thinking blocks from a reply's `reasoning_details` items, as they come:
/// a block is whole once an item carries its signature (text arriving in
/// items before it is the same block's); an encrypted item is a block of
/// its own; the reply's block order (`reasoning.order`) is kept as it came.
/// Items in another provider's format are not Claude's, and are dropped.
#[derive(Default)]
pub(super) struct ReasoningBlocks {
    pending: String,
}

impl ReasoningBlocks {
    pub(super) fn absorb(&mut self, items: &[Value]) -> Vec<ThinkingBlock> {
        let mut out = Vec::new();
        for item in items {
            let format = item
                .get("format")
                .and_then(|f| f.as_str())
                .unwrap_or_default();
            if !format.is_empty() && format != CLAUDE_FORMAT {
                continue;
            }
            match item.get("type").and_then(|t| t.as_str()) {
                Some("reasoning.text") => {
                    if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                        self.pending.push_str(text);
                    }
                    if let Some(signature) = item
                        .get("signature")
                        .and_then(|s| s.as_str())
                        .filter(|s| !s.is_empty())
                    {
                        out.push(ThinkingBlock::Thinking {
                            thinking: std::mem::take(&mut self.pending),
                            signature: signature.to_string(),
                        });
                    }
                }
                Some("reasoning.encrypted") => {
                    if let Some(data) = item.get("data").and_then(|d| d.as_str()) {
                        out.push(ThinkingBlock::RedactedThinking {
                            data: data.to_string(),
                        });
                    }
                }
                Some("reasoning.order") => {
                    if let Some(blocks) = item
                        .get("blocks")
                        .and_then(|b| b.as_array())
                        .filter(|b| !b.is_empty())
                    {
                        out.push(ThinkingBlock::BlockOrder {
                            blocks: blocks.clone(),
                        });
                    }
                }
                _ => {}
            }
        }
        out
    }
}

/// A tool result as `build_messages` reads it, with the deferred tools it
/// loaded.
#[derive(serde::Deserialize)]
struct LoadingResult {
    tool_call_id: String,
    content: String,
    #[serde(default, rename = "loadedTools")]
    loaded_tools: Vec<Value>,
}

/// Apply the extensions to `body`, the serialized request: `system_marked`
/// leading system messages are stable chunks, `thinking` the signed blocks
/// of the assistant messages by their index in `body.messages`.
pub(super) fn apply(
    body: &mut Value,
    req: &ChatRequest,
    system_marked: usize,
    thinking: &HashMap<usize, Vec<ThinkingBlock>>,
) {
    let cached = !req.cache_breakpoints.is_empty();
    let Some(map) = body.as_object_mut() else {
        return;
    };

    if req.enable_thinking {
        map.insert(
            "reasoning".to_string(),
            json!({"max_tokens": thinking_budget(req.effort)}),
        );
        map.remove("reasoning_effort");
    }

    // Results that loaded deferred tools the model may reference.
    // Janus loads them by reference on the models that read references and
    // falls back to a plain line on the older ones, so a reference is
    // always sent.
    let referable: HashSet<&str> = req.tools.iter().map(|t| t.name.as_str()).collect();
    let mut loads: HashMap<String, (String, Vec<crate::types::ToolDefinition>)> = HashMap::new();
    for msg in req.messages.iter().filter(|m| m.role == "tool") {
        let Some(results) = msg
            .tool_results
            .clone()
            .and_then(|v| serde_json::from_value::<Vec<LoadingResult>>(v).ok())
        else {
            continue;
        };
        for r in results {
            let defs: Vec<_> = r.loaded_tools.iter().filter_map(definition_of).collect();
            if !defs.is_empty() && defs.iter().all(|d| referable.contains(d.name.as_str())) {
                loads.insert(r.tool_call_id, (r.content, defs));
            }
        }
    }

    let mut referenced: HashSet<String> = HashSet::new();
    let mut system_markers = 0usize;
    if let Some(messages) = map.get_mut("messages").and_then(|m| m.as_array_mut()) {
        for (i, msg) in messages.iter_mut().enumerate() {
            let role = msg
                .get("role")
                .and_then(|r| r.as_str())
                .unwrap_or_default()
                .to_string();
            match role.as_str() {
                // The stable chunks; the last three when there are more, so
                // the longest prefix stays cached and the tools and last
                // message keep their markers.
                "system"
                    if cached
                        && i < system_marked
                        && system_marked - i <= MAX_CACHE_MARKERS - 1 =>
                {
                    if let Some(text) = msg
                        .get("content")
                        .and_then(|c| c.as_str())
                        .map(String::from)
                    {
                        msg["content"] =
                            json!([{"type": "text", "text": text, "cache_control": marker()}]);
                        system_markers += 1;
                    }
                }
                "assistant" => {
                    if let Some(blocks) = thinking.get(&i).filter(|b| !b.is_empty()) {
                        msg["reasoning_details"] = reasoning_details(blocks);
                    }
                }
                "tool" => {
                    let id = msg
                        .get("tool_call_id")
                        .and_then(|t| t.as_str())
                        .unwrap_or_default()
                        .to_string();
                    if let Some((content, defs)) = loads.get(&id) {
                        let text = content.replacen(&functions_block(defs), "", 1);
                        let mut parts = Vec::new();
                        if !text.trim().is_empty() {
                            parts.push(json!({"type": "text", "text": text.trim()}));
                        }
                        for d in defs {
                            referenced.insert(d.name.clone());
                            parts.push(json!({"type": "tool_reference", "tool_name": d.name}));
                        }
                        msg["content"] = Value::Array(parts);
                    }
                }
                _ => {}
            }
        }
        // One marker on the last message's last part: each step reads the
        // previous step's whole prefix from the cache.
        if cached && let Some(last) = messages.last_mut() {
            match last.get("content") {
                Some(Value::String(text)) => {
                    let text = text.clone();
                    last["content"] =
                        json!([{"type": "text", "text": text, "cache_control": marker()}]);
                }
                Some(Value::Array(_)) => {
                    if let Some(part) = last["content"]
                        .as_array_mut()
                        .and_then(|p| p.last_mut())
                        .and_then(|p| p.as_object_mut())
                    {
                        part.insert("cache_control".to_string(), marker());
                    }
                }
                _ => {}
            }
        }
    }

    // At least one tool is loaded up front, or Anthropic refuses the request.
    let deferred = if referenced.len() < req.tools.len() {
        referenced
    } else {
        HashSet::new()
    };
    if let Some(tools) = map.get_mut("tools").and_then(|t| t.as_array_mut()) {
        let name = |t: &Value| {
            t.pointer("/function/name")
                .and_then(|n| n.as_str())
                .unwrap_or_default()
                .to_string()
        };
        let last_loaded = tools
            .iter()
            .rposition(|t| !deferred.contains(&name(t)))
            .filter(|_| cached && system_markers + 2 <= MAX_CACHE_MARKERS);
        for (i, tool) in tools.iter_mut().enumerate() {
            if deferred.contains(&name(tool)) {
                tool["defer_loading"] = json!(true);
            }
            if Some(i) == last_loaded {
                tool["cache_control"] = marker();
            }
        }
    }
}
