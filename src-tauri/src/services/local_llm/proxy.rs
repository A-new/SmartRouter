// Anthropic ↔ OpenAI format conversion.
//
// Salvaged from the old unified local-LLM proxy: the bundled llama-server and
// its HTTP proxy are gone, but the Smart Router still needs these pure
// request/response transforms to expose an Anthropic-compatible endpoint in
// front of an OpenAI-only upstream.

// ─── Format Conversion: Anthropic ↔ OpenAI ───

use std::collections::HashMap;

fn anthropic_user_content_part(block: &serde_json::Value) -> Option<serde_json::Value> {
    match block.get("type").and_then(|value| value.as_str()) {
        Some("text") => block
            .get("text")
            .and_then(|value| value.as_str())
            .map(|text| serde_json::json!({"type": "text", "text": text})),
        Some("image") => {
            let source = block.get("source")?;
            let url = match source.get("type").and_then(|value| value.as_str()) {
                Some("base64") => {
                    let media_type = source.get("media_type")?.as_str()?;
                    let data = source.get("data")?.as_str()?;
                    format!("data:{media_type};base64,{data}")
                }
                Some("url") => source.get("url")?.as_str()?.to_string(),
                _ => return None,
            };
            Some(serde_json::json!({
                "type": "image_url",
                "image_url": {"url": url}
            }))
        }
        Some("document") => {
            let source = block.get("source")?;
            match source.get("type").and_then(|value| value.as_str()) {
                Some("base64") => {
                    let data = source.get("data")?.as_str()?;
                    let filename = block
                        .get("title")
                        .and_then(|value| value.as_str())
                        .filter(|title| !title.trim().is_empty())
                        .unwrap_or("document.pdf");
                    Some(serde_json::json!({
                        "type": "file",
                        "file": {"file_data": data, "filename": filename}
                    }))
                }
                Some("text") => source
                    .get("data")
                    .and_then(|value| value.as_str())
                    .map(|text| serde_json::json!({"type": "text", "text": text})),
                Some("url") => source
                    .get("url")
                    .and_then(|value| value.as_str())
                    .map(|url| {
                        serde_json::json!({
                            "type": "text",
                            "text": format!("Document URL: {url}")
                        })
                    }),
                Some("file") => {
                    source
                        .get("file_id")
                        .and_then(|value| value.as_str())
                        .map(|file_id| {
                            serde_json::json!({
                                "type": "text",
                                "text": format!("Anthropic file reference: {file_id}")
                            })
                        })
                }
                _ => None,
            }
        }
        _ => None,
    }
}

fn openai_user_content(parts: Vec<serde_json::Value>) -> serde_json::Value {
    let text_parts: Option<Vec<&str>> = parts
        .iter()
        .map(|part| {
            if part.get("type").and_then(|value| value.as_str()) == Some("text") {
                part.get("text").and_then(|value| value.as_str())
            } else {
                None
            }
        })
        .collect();

    match text_parts {
        Some(text_parts) => serde_json::Value::String(text_parts.concat()),
        None => serde_json::Value::Array(parts),
    }
}

fn anthropic_tool_result_content(
    tool_result: &serde_json::Value,
) -> (String, Vec<serde_json::Value>) {
    let Some(content) = tool_result.get("content") else {
        return (String::new(), Vec::new());
    };
    if let Some(text) = content.as_str() {
        return (text.to_string(), Vec::new());
    }

    let mut text = String::new();
    let mut attachments = Vec::new();
    if let Some(blocks) = content.as_array() {
        for block in blocks {
            if block.get("type").and_then(|value| value.as_str()) == Some("text") {
                if let Some(part) = block.get("text").and_then(|value| value.as_str()) {
                    text.push_str(part);
                }
            } else if let Some(part) = anthropic_user_content_part(block) {
                attachments.push(part);
            }
        }
    }
    (text, attachments)
}

/// Convert Anthropic Messages request → OpenAI Chat Completions request
pub(crate) fn anthropic_to_openai(body: &serde_json::Value) -> serde_json::Value {
    let mut messages = Vec::new();

    // System message
    if let Some(system) = body.get("system") {
        let system_text = if let Some(s) = system.as_str() {
            s.to_string()
        } else if let Some(arr) = system.as_array() {
            arr.iter()
                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("")
        } else {
            String::new()
        };
        if !system_text.is_empty() {
            messages.push(serde_json::json!({"role": "system", "content": system_text}));
        }
    }

    // Conversation messages — handle text, tool_use, and tool_result blocks
    if let Some(msgs) = body.get("messages").and_then(|m| m.as_array()) {
        for msg in msgs {
            let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("user");

            match msg.get("content") {
                // Simple string content
                Some(c) if c.is_string() => {
                    messages.push(
                        serde_json::json!({"role": role, "content": c.as_str().unwrap_or("")}),
                    );
                }
                // Array of content blocks
                Some(c) if c.is_array() => {
                    let blocks = c.as_array().unwrap();

                    let text_parts: Vec<&str> = blocks
                        .iter()
                        .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
                        .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                        .collect();

                    let tool_use_blocks: Vec<&serde_json::Value> = blocks
                        .iter()
                        .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_use"))
                        .collect();

                    let tool_result_blocks: Vec<&serde_json::Value> = blocks
                        .iter()
                        .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_result"))
                        .collect();

                    if !tool_use_blocks.is_empty() {
                        let tool_calls: Vec<serde_json::Value> = tool_use_blocks.iter().map(|tu| {
                            let id = tu.get("id").and_then(|v| v.as_str()).unwrap_or("call_0");
                            let name = tu.get("name").and_then(|v| v.as_str()).unwrap_or("");
                            let input = tu.get("input").cloned().unwrap_or(serde_json::json!({}));
                            serde_json::json!({
                                "id": id, "type": "function",
                                "function": {
                                    "name": name,
                                    "arguments": serde_json::to_string(&input).unwrap_or_else(|_| "{}".to_string())
                                }
                            })
                        }).collect();
                        let text_content = text_parts.join("");
                        let mut assistant_msg = serde_json::json!({
                            "role": "assistant",
                            "tool_calls": tool_calls
                        });
                        if !text_content.is_empty() {
                            assistant_msg["content"] = serde_json::json!(text_content);
                        }
                        messages.push(assistant_msg);
                    } else if !tool_result_blocks.is_empty() {
                        let mut follow_up_content = Vec::new();
                        for tr in &tool_result_blocks {
                            let tool_call_id =
                                tr.get("tool_use_id").and_then(|v| v.as_str()).unwrap_or("");
                            let (mut result_content, attachments) =
                                anthropic_tool_result_content(tr);
                            if result_content.is_empty() && !attachments.is_empty() {
                                result_content =
                                    "Attachments are included in the following user message."
                                        .to_string();
                            }
                            messages.push(serde_json::json!({
                                "role": "tool",
                                "tool_call_id": tool_call_id,
                                "content": result_content
                            }));
                            if !attachments.is_empty() {
                                follow_up_content.push(serde_json::json!({
                                    "type": "text",
                                    "text": format!("Attachments from tool result {tool_call_id}:")
                                }));
                                follow_up_content.extend(attachments);
                            }
                        }
                        follow_up_content.extend(
                            blocks
                                .iter()
                                .filter(|block| {
                                    !matches!(
                                        block.get("type").and_then(|value| value.as_str()),
                                        Some("tool_result" | "tool_use")
                                    )
                                })
                                .filter_map(anthropic_user_content_part),
                        );
                        if !follow_up_content.is_empty() {
                            messages.push(serde_json::json!({
                                "role": role,
                                "content": openai_user_content(follow_up_content)
                            }));
                        }
                    } else {
                        let content: Vec<serde_json::Value> = blocks
                            .iter()
                            .filter_map(anthropic_user_content_part)
                            .collect();
                        messages.push(serde_json::json!({
                            "role": role,
                            "content": openai_user_content(content)
                        }));
                    }
                }
                _ => {
                    messages.push(serde_json::json!({"role": role, "content": ""}));
                }
            }
        }
    }

    let mut req = serde_json::json!({
        "model": body.get("model").and_then(|m| m.as_str()).unwrap_or("local-model"),
        "messages": messages,
        "max_tokens": body.get("max_tokens").and_then(|v| v.as_u64()).unwrap_or(4096),
        "stream": body.get("stream").and_then(|v| v.as_bool()).unwrap_or(false),
    });

    if let Some(temperature) = body.get("temperature") {
        req["temperature"] = temperature.clone();
    }

    if let Some(tools) = body.get("tools").and_then(|t| t.as_array()) {
        if !tools.is_empty() {
            let openai_tools: Vec<serde_json::Value> = tools.iter().map(|t| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                        "description": t.get("description").and_then(|v| v.as_str()).unwrap_or(""),
                        "parameters": t.get("input_schema").cloned()
                            .unwrap_or(serde_json::json!({"type": "object", "properties": {}}))
                    }
                })
            }).collect();
            req["tools"] = serde_json::json!(openai_tools);
            if let Some(tc) = body.get("tool_choice") {
                let choice_type = tc.get("type").and_then(|v| v.as_str()).unwrap_or("auto");
                req["tool_choice"] = match choice_type {
                    "any" => serde_json::json!("required"),
                    "none" => serde_json::json!("none"),
                    "tool" => serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": tc.get("name").and_then(|v| v.as_str()).unwrap_or("")
                        }
                    }),
                    _ => serde_json::json!("auto"),
                };
                if tc
                    .get("disable_parallel_tool_use")
                    .and_then(|v| v.as_bool())
                    == Some(true)
                {
                    req["parallel_tool_calls"] = serde_json::json!(false);
                }
            }
        }
    }

    if let Some(stop) = body.get("stop_sequences") {
        req["stop"] = stop.clone();
    }
    if let Some(top_p) = body.get("top_p") {
        req["top_p"] = top_p.clone();
    }

    req
}

/// Convert OpenAI Chat Completions non-streaming response → Anthropic Messages response
pub(crate) fn openai_to_anthropic(data: &serde_json::Value) -> serde_json::Value {
    let model = data
        .get("model")
        .and_then(|m| m.as_str())
        .unwrap_or("local-model");
    let choice = data
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first());
    let message = choice.and_then(|c| c.get("message"));
    let finish_reason = choice
        .and_then(|c| c.get("finish_reason"))
        .and_then(|v| v.as_str())
        .unwrap_or("end_turn");

    let mut content_blocks: Vec<serde_json::Value> = Vec::new();

    if let Some(text) = message
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
    {
        if !text.is_empty() {
            content_blocks.push(serde_json::json!({"type": "text", "text": text}));
        }
    }

    if let Some(tool_calls) = message
        .and_then(|m| m.get("tool_calls"))
        .and_then(|t| t.as_array())
    {
        for tc in tool_calls {
            let id = tc.get("id").and_then(|v| v.as_str()).unwrap_or("call_0");
            let name = tc
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let args_str = tc
                .get("function")
                .and_then(|f| f.get("arguments"))
                .and_then(|v| v.as_str())
                .unwrap_or("{}");
            let input: serde_json::Value =
                serde_json::from_str(args_str).unwrap_or(serde_json::json!({}));
            content_blocks.push(serde_json::json!({
                "type": "tool_use", "id": id, "name": name, "input": input
            }));
        }
    }

    if content_blocks.is_empty() {
        content_blocks.push(serde_json::json!({"type": "text", "text": ""}));
    }

    let stop_reason = match finish_reason {
        "tool_calls" => "tool_use",
        "stop" | "end_turn" => "end_turn",
        "length" => "max_tokens",
        other => other,
    };

    serde_json::json!({
        "id": format!("msg_{}", chrono::Utc::now().timestamp_millis()),
        "type": "message",
        "role": "assistant",
        "content": content_blocks,
        "model": model,
        "stop_reason": stop_reason,
        "stop_sequence": null,
        "usage": {
            "input_tokens": data.get("usage").and_then(|u| u.get("prompt_tokens")).and_then(|v| v.as_u64()).unwrap_or(0),
            "output_tokens": data.get("usage").and_then(|u| u.get("completion_tokens")).and_then(|v| v.as_u64()).unwrap_or(0),
        }
    })
}

// ─── Reverse direction: OpenAI request → Anthropic request ───

/// Convert an OpenAI Chat Completions request → Anthropic Messages request.
///
/// The router keeps every conversation in the OpenAI shape — its Anthropic
/// endpoint is translated up front by [`anthropic_to_openai`] — so a candidate
/// that only exposes an Anthropic URL needs this reverse hop on the way out.
///
/// `model` is the upstream model name the caller resolved from the candidate;
/// the request's own `model` field is ignored, exactly like the OpenAI path.
pub(crate) fn openai_to_anthropic_request(
    body: &serde_json::Value,
    model: &str,
) -> serde_json::Value {
    let mut system_parts: Vec<String> = Vec::new();
    let mut messages: Vec<serde_json::Value> = Vec::new();
    // OpenAI reports tool output as its own `role: "tool"` message, while
    // Anthropic carries `tool_result` blocks inside the next user turn, so they
    // are buffered until that turn shows up.
    let mut pending_tool_results: Vec<serde_json::Value> = Vec::new();

    let openai_messages = body
        .get("messages")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();

    for message in &openai_messages {
        let role = message
            .get("role")
            .and_then(|value| value.as_str())
            .unwrap_or("user");

        if role == "system" || role == "developer" {
            if let Some(text) = openai_text_content(message.get("content")) {
                if !text.is_empty() {
                    system_parts.push(text);
                }
            }
            continue;
        }

        if role == "tool" {
            let tool_use_id = message
                .get("tool_call_id")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            if !tool_use_id.is_empty() {
                pending_tool_results.push(serde_json::json!({
                    "type": "tool_result",
                    "tool_use_id": tool_use_id,
                    "content": openai_text_content(message.get("content")).unwrap_or_default(),
                }));
            }
            continue;
        }

        let mut blocks = Vec::new();
        if role == "assistant" {
            if let Some(text) = openai_text_content(message.get("content")) {
                if !text.is_empty() {
                    blocks.push(serde_json::json!({"type": "text", "text": text}));
                }
            }
            for call in message
                .get("tool_calls")
                .and_then(|value| value.as_array())
                .into_iter()
                .flatten()
            {
                let input = call
                    .pointer("/function/arguments")
                    .and_then(|value| value.as_str())
                    .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
                    .unwrap_or_else(|| serde_json::json!({}));
                blocks.push(serde_json::json!({
                    "type": "tool_use",
                    "id": call
                        .get("id")
                        .and_then(|value| value.as_str())
                        .unwrap_or("call_0"),
                    "name": call
                        .pointer("/function/name")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default(),
                    "input": input,
                }));
            }
        } else {
            blocks.append(&mut pending_tool_results);
            blocks.extend(openai_user_content_blocks(message.get("content")));
        }

        push_anthropic_message(&mut messages, role, blocks);
    }

    // Tool output with no following user turn still has to be delivered, or the
    // model never sees the result it asked for.
    if !pending_tool_results.is_empty() {
        push_anthropic_message(&mut messages, "user", pending_tool_results);
    }
    if messages.is_empty() {
        messages.push(serde_json::json!({"role": "user", "content": ""}));
    }

    let max_tokens = body
        .get("max_tokens")
        .or_else(|| body.get("max_completion_tokens"))
        .and_then(|value| value.as_u64())
        .unwrap_or(4096);

    let mut request = serde_json::json!({
        "model": model,
        "messages": messages,
        // Anthropic requires it; OpenAI treats it as optional.
        "max_tokens": max_tokens,
    });

    if !system_parts.is_empty() {
        request["system"] = serde_json::json!(system_parts.join("\n\n"));
    }
    if let Some(stream) = body.get("stream").and_then(|value| value.as_bool()) {
        request["stream"] = serde_json::json!(stream);
    }
    for key in ["temperature", "top_p"] {
        if let Some(value) = body.get(key).filter(|value| !value.is_null()) {
            request[key] = value.clone();
        }
    }
    match body.get("stop") {
        Some(serde_json::Value::String(sequence)) if !sequence.is_empty() => {
            request["stop_sequences"] = serde_json::json!([sequence]);
        }
        Some(serde_json::Value::Array(sequences)) if !sequences.is_empty() => {
            request["stop_sequences"] = serde_json::json!(sequences);
        }
        _ => {}
    }
    if let Some(tools) = body.get("tools").and_then(|value| value.as_array()) {
        let anthropic_tools: Vec<serde_json::Value> = tools
            .iter()
            .filter_map(|tool| {
                let function = tool.get("function")?;
                Some(serde_json::json!({
                    "name": function
                        .get("name")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default(),
                    "description": function
                        .get("description")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default(),
                    "input_schema": function.get("parameters").cloned().unwrap_or_else(
                        || serde_json::json!({"type": "object", "properties": {}})
                    ),
                }))
            })
            .collect();
        if !anthropic_tools.is_empty() {
            request["tools"] = serde_json::json!(anthropic_tools);
            if let Some(choice) = body.get("tool_choice") {
                request["tool_choice"] = openai_tool_choice_to_anthropic(
                    choice,
                    body.get("parallel_tool_calls")
                        .and_then(|value| value.as_bool()),
                );
            }
        }
    }

    request
}

/// Flatten an OpenAI `content` field (bare string or parts array) down to text.
fn openai_text_content(content: Option<&serde_json::Value>) -> Option<String> {
    match content? {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Array(parts) => Some(
            parts
                .iter()
                .filter(|part| part.get("type").and_then(|value| value.as_str()) == Some("text"))
                .filter_map(|part| part.get("text").and_then(|value| value.as_str()))
                .collect::<String>(),
        ),
        _ => None,
    }
}

/// OpenAI content parts → Anthropic content blocks (text and inline images).
fn openai_user_content_blocks(content: Option<&serde_json::Value>) -> Vec<serde_json::Value> {
    match content {
        Some(serde_json::Value::String(text)) if !text.is_empty() => {
            vec![serde_json::json!({"type": "text", "text": text})]
        }
        Some(serde_json::Value::Array(parts)) => {
            parts.iter().filter_map(openai_part_to_anthropic).collect()
        }
        _ => Vec::new(),
    }
}

fn openai_part_to_anthropic(part: &serde_json::Value) -> Option<serde_json::Value> {
    match part.get("type").and_then(|value| value.as_str()) {
        Some("text") => Some(serde_json::json!({
            "type": "text",
            "text": part.get("text").and_then(|value| value.as_str()).unwrap_or_default(),
        })),
        Some("image_url") => {
            let url = part
                .pointer("/image_url/url")
                .and_then(|value| value.as_str())?;
            // Anthropic's API takes base64 bytes or a provider file id, never a
            // remote URL, so anything that is not an inline data URL is dropped.
            let rest = url.strip_prefix("data:")?;
            let (media_type, data) = rest.split_once(";base64,")?;
            Some(serde_json::json!({
                "type": "image",
                "source": {"type": "base64", "media_type": media_type, "data": data},
            }))
        }
        _ => None,
    }
}

/// Normalise a message body into Anthropic content blocks, so merging two turns
/// never mixes a bare string with block objects.
fn anthropic_content_blocks(content: serde_json::Value) -> Vec<serde_json::Value> {
    match content {
        serde_json::Value::String(text) if !text.is_empty() => {
            vec![serde_json::json!({"type": "text", "text": text})]
        }
        serde_json::Value::Array(blocks) => blocks,
        _ => Vec::new(),
    }
}

/// Append a message, merging into the previous turn when the roles match:
/// Anthropic requires alternating roles and rejects two user turns in a row.
fn push_anthropic_message(
    messages: &mut Vec<serde_json::Value>,
    role: &str,
    blocks: Vec<serde_json::Value>,
) {
    if blocks.is_empty() {
        return;
    }
    if let Some(last) = messages.last_mut() {
        if last.get("role").and_then(|value| value.as_str()) == Some(role) {
            let mut merged = last
                .get("content")
                .cloned()
                .map(anthropic_content_blocks)
                .unwrap_or_default();
            merged.extend(blocks);
            *last = serde_json::json!({"role": role, "content": merged});
            return;
        }
    }
    messages.push(serde_json::json!({"role": role, "content": blocks}));
}

fn openai_tool_choice_to_anthropic(
    choice: &serde_json::Value,
    parallel_tool_calls: Option<bool>,
) -> serde_json::Value {
    let mut converted = match choice {
        serde_json::Value::String(name) => match name.as_str() {
            "required" => serde_json::json!({"type": "any"}),
            "none" => serde_json::json!({"type": "none"}),
            _ => serde_json::json!({"type": "auto"}),
        },
        serde_json::Value::Object(_) => serde_json::json!({
            "type": "tool",
            "name": choice
                .pointer("/function/name")
                .and_then(|value| value.as_str())
                .unwrap_or_default(),
        }),
        _ => serde_json::json!({"type": "auto"}),
    };
    if parallel_tool_calls == Some(false) {
        converted["disable_parallel_tool_use"] = serde_json::json!(true);
    }
    converted
}

// ─── Reverse direction: Anthropic response → OpenAI response ───

/// Convert an Anthropic Messages response → OpenAI Chat Completions response.
pub(crate) fn anthropic_to_openai_response(
    data: &serde_json::Value,
    fallback_model: &str,
) -> serde_json::Value {
    let mut text = String::new();
    let mut tool_calls = Vec::new();

    for block in data
        .get("content")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
    {
        match block.get("type").and_then(|value| value.as_str()) {
            Some("text") => {
                if let Some(part) = block.get("text").and_then(|value| value.as_str()) {
                    text.push_str(part);
                }
            }
            Some("tool_use") => tool_calls.push(serde_json::json!({
                "id": block.get("id").and_then(|value| value.as_str()).unwrap_or("call_0"),
                "type": "function",
                "function": {
                    "name": block.get("name").and_then(|value| value.as_str()).unwrap_or_default(),
                    "arguments": serde_json::to_string(
                        block.get("input").unwrap_or(&serde_json::json!({}))
                    )
                    .unwrap_or_else(|_| "{}".to_string()),
                },
            })),
            _ => {}
        }
    }

    let finish_reason = match data.get("stop_reason").and_then(|value| value.as_str()) {
        Some("tool_use") => "tool_calls",
        Some("max_tokens") => "length",
        _ => "stop",
    };

    let mut message = serde_json::json!({
        "role": "assistant",
        "content": if text.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::json!(text)
        },
    });
    if !tool_calls.is_empty() {
        message["tool_calls"] = serde_json::json!(tool_calls);
    }

    let prompt_tokens = data
        .pointer("/usage/input_tokens")
        .and_then(|value| value.as_u64())
        .unwrap_or(0);
    let completion_tokens = data
        .pointer("/usage/output_tokens")
        .and_then(|value| value.as_u64())
        .unwrap_or(0);

    serde_json::json!({
        "id": data
            .get("id")
            .and_then(|value| value.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| format!("chatcmpl-{}", chrono::Utc::now().timestamp_millis())),
        "object": "chat.completion",
        "created": chrono::Utc::now().timestamp(),
        "model": data
            .get("model")
            .and_then(|value| value.as_str())
            .unwrap_or(fallback_model),
        "choices": [{
            "index": 0,
            "message": message,
            "finish_reason": finish_reason,
        }],
        "usage": {
            "prompt_tokens": prompt_tokens,
            "completion_tokens": completion_tokens,
            "total_tokens": prompt_tokens + completion_tokens,
        }
    })
}

/// Anthropic Messages SSE → OpenAI Chat Completions SSE.
///
/// The mirror image of the router's `AnthropicSseAdapter`: raw upstream bytes go
/// in, whole OpenAI frames come out, and events split across TCP chunks are held
/// back until they are complete. Frames are returned as strings so this stays a
/// pure transform the caller can wrap in whatever body type it streams.
pub(crate) struct AnthropicToOpenAiStream {
    model: String,
    id: String,
    created: i64,
    buffer: String,
    started: bool,
    finished: bool,
    /// Anthropic content-block index → OpenAI `tool_calls` index.
    tool_indexes: HashMap<u64, usize>,
    next_tool_index: usize,
    stop_reason: Option<String>,
}

impl AnthropicToOpenAiStream {
    pub(crate) fn new(model: &str) -> Self {
        Self {
            model: model.to_string(),
            id: format!("chatcmpl-{}", chrono::Utc::now().timestamp_millis()),
            created: chrono::Utc::now().timestamp(),
            buffer: String::new(),
            started: false,
            finished: false,
            tool_indexes: HashMap::new(),
            next_tool_index: 0,
            stop_reason: None,
        }
    }

    /// Feed one upstream chunk, returning every complete frame it completes.
    pub(crate) fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buffer.push_str(&String::from_utf8_lossy(chunk));
        let mut frames = Vec::new();
        while let Some(separator) = self.buffer.find("\n\n") {
            let event = self.buffer[..separator].to_string();
            self.buffer.drain(..separator + 2);
            self.handle_event(&event, &mut frames);
        }
        frames
    }

    /// Close out a stream that ended without a `message_stop`.
    pub(crate) fn finish(&mut self) -> Vec<String> {
        let mut frames = Vec::new();
        if !self.finished {
            self.emit_start(&mut frames);
            self.emit_final(&mut frames);
        }
        frames
    }

    fn handle_event(&mut self, event: &str, frames: &mut Vec<String>) {
        let Some(data) = event
            .lines()
            .find_map(|line| line.strip_prefix("data:").map(str::trim))
        else {
            return;
        };
        if data.is_empty() || data == "[DONE]" {
            return;
        }
        let Ok(payload) = serde_json::from_str::<serde_json::Value>(data) else {
            return;
        };

        match payload.get("type").and_then(|value| value.as_str()) {
            Some("message_start") => {
                if let Some(model) = payload
                    .pointer("/message/model")
                    .and_then(|value| value.as_str())
                {
                    self.model = model.to_string();
                }
                if let Some(id) = payload
                    .pointer("/message/id")
                    .and_then(|value| value.as_str())
                {
                    self.id = id.to_string();
                }
                self.emit_start(frames);
            }
            Some("content_block_start") => {
                let index = payload
                    .get("index")
                    .and_then(|value| value.as_u64())
                    .unwrap_or(0);
                let block = payload.get("content_block");
                if block
                    .and_then(|value| value.get("type"))
                    .and_then(|value| value.as_str())
                    == Some("tool_use")
                {
                    let tool_index = self.take_tool_index(index);
                    self.emit_start(frames);
                    frames.push(self.frame(serde_json::json!({
                        "tool_calls": [{
                            "index": tool_index,
                            "id": block
                                .and_then(|value| value.get("id"))
                                .and_then(|value| value.as_str())
                                .unwrap_or("call_0"),
                            "type": "function",
                            "function": {
                                "name": block
                                    .and_then(|value| value.get("name"))
                                    .and_then(|value| value.as_str())
                                    .unwrap_or_default(),
                                "arguments": "",
                            }
                        }]
                    })));
                }
            }
            Some("content_block_delta") => {
                let index = payload
                    .get("index")
                    .and_then(|value| value.as_u64())
                    .unwrap_or(0);
                let delta = payload.get("delta");
                match delta
                    .and_then(|value| value.get("type"))
                    .and_then(|value| value.as_str())
                {
                    Some("text_delta") => {
                        let text = delta
                            .and_then(|value| value.get("text"))
                            .and_then(|value| value.as_str())
                            .unwrap_or_default();
                        if !text.is_empty() {
                            self.emit_start(frames);
                            frames.push(self.frame(serde_json::json!({"content": text})));
                        }
                    }
                    Some("input_json_delta") => {
                        let partial = delta
                            .and_then(|value| value.get("partial_json"))
                            .and_then(|value| value.as_str())
                            .unwrap_or_default();
                        if !partial.is_empty() {
                            let tool_index = self.take_tool_index(index);
                            self.emit_start(frames);
                            frames.push(self.frame(serde_json::json!({
                                "tool_calls": [{
                                    "index": tool_index,
                                    "function": {"arguments": partial},
                                }]
                            })));
                        }
                    }
                    // `thinking_delta` and `signature_delta` carry extended
                    // thinking, which has no OpenAI chunk equivalent.
                    _ => {}
                }
            }
            Some("message_delta") => {
                if let Some(reason) = payload
                    .pointer("/delta/stop_reason")
                    .and_then(|value| value.as_str())
                {
                    self.stop_reason = Some(reason.to_string());
                }
            }
            Some("message_stop") => {
                self.emit_start(frames);
                self.emit_final(frames);
            }
            Some("error") => {
                let message = payload
                    .pointer("/error/message")
                    .and_then(|value| value.as_str())
                    .unwrap_or("Upstream Anthropic stream failed");
                frames.push(format!(
                    "data: {}\n\n",
                    serde_json::json!({"error": {"message": message}})
                ));
                self.emit_final(frames);
            }
            _ => {}
        }
    }

    /// Reserve (or recall) the OpenAI tool index for an Anthropic block index.
    fn take_tool_index(&mut self, index: u64) -> usize {
        if let Some(existing) = self.tool_indexes.get(&index) {
            return *existing;
        }
        let assigned = self.next_tool_index;
        self.next_tool_index += 1;
        self.tool_indexes.insert(index, assigned);
        assigned
    }

    /// OpenAI clients expect the opening chunk to announce the role before any
    /// content arrives, and upstreams are not consistent about emitting
    /// `message_start`, so this is idempotent and called from every entry point.
    fn emit_start(&mut self, frames: &mut Vec<String>) {
        if self.started {
            return;
        }
        self.started = true;
        frames.push(self.frame_with_finish(
            serde_json::json!({"role": "assistant", "content": ""}),
            serde_json::Value::Null,
        ));
    }

    fn emit_final(&mut self, frames: &mut Vec<String>) {
        if self.finished {
            return;
        }
        self.finished = true;
        let finish_reason = match self.stop_reason.as_deref() {
            Some("tool_use") => "tool_calls",
            Some("max_tokens") => "length",
            _ => "stop",
        };
        frames
            .push(self.frame_with_finish(serde_json::json!({}), serde_json::json!(finish_reason)));
        frames.push("data: [DONE]\n\n".to_string());
    }

    fn frame(&self, delta: serde_json::Value) -> String {
        self.frame_with_finish(delta, serde_json::Value::Null)
    }

    fn frame_with_finish(
        &self,
        delta: serde_json::Value,
        finish_reason: serde_json::Value,
    ) -> String {
        let chunk = serde_json::json!({
            "id": self.id,
            "object": "chat.completion.chunk",
            "created": self.created,
            "model": self.model,
            "choices": [{
                "index": 0,
                "delta": delta,
                "finish_reason": finish_reason,
            }]
        });
        format!("data: {chunk}\n\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_openai_requests_back_to_anthropic_shape() {
        let converted = openai_to_anthropic_request(
            &serde_json::json!({
                "model": "ignored",
                "max_tokens": 128,
                "stream": true,
                "stop": "END",
                "messages": [
                    {"role": "system", "content": "be terse"},
                    {"role": "user", "content": "hi"},
                    {"role": "assistant", "content": null, "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {"name": "weather", "arguments": "{\"city\":\"sf\"}"}
                    }]},
                    {"role": "tool", "tool_call_id": "call_1", "content": "sunny"}
                ],
                "tools": [{
                    "type": "function",
                    "function": {"name": "weather", "description": "d", "parameters": {"type": "object"}}
                }],
                "tool_choice": "required"
            }),
            "claude-upstream",
        );

        assert_eq!(converted["model"], "claude-upstream");
        assert_eq!(converted["system"], "be terse");
        assert_eq!(converted["max_tokens"], 128);
        assert_eq!(converted["stream"], true);
        assert_eq!(converted["stop_sequences"], serde_json::json!(["END"]));
        assert_eq!(converted["tools"][0]["name"], "weather");
        assert_eq!(converted["tools"][0]["input_schema"]["type"], "object");
        assert_eq!(converted["tool_choice"], serde_json::json!({"type": "any"}));

        // user → assistant(tool_use) → user(tool_result), with the system turn
        // hoisted out of the conversation entirely.
        assert_eq!(converted["messages"].as_array().unwrap().len(), 3);
        assert_eq!(converted["messages"][0]["role"], "user");
        assert_eq!(converted["messages"][1]["content"][0]["type"], "tool_use");
        assert_eq!(
            converted["messages"][1]["content"][0]["input"]["city"],
            "sf"
        );
        assert_eq!(converted["messages"][2]["role"], "user");
        assert_eq!(
            converted["messages"][2]["content"][0]["type"],
            "tool_result"
        );
        assert_eq!(
            converted["messages"][2]["content"][0]["tool_use_id"],
            "call_1"
        );
        assert_eq!(converted["messages"][2]["content"][0]["content"], "sunny");
    }

    #[test]
    fn merges_consecutive_openai_turns_of_the_same_role() {
        let converted = openai_to_anthropic_request(
            &serde_json::json!({
                "messages": [
                    {"role": "user", "content": "one"},
                    {"role": "user", "content": "two"}
                ]
            }),
            "model",
        );

        // Anthropic rejects two user turns in a row.
        assert_eq!(converted["messages"].as_array().unwrap().len(), 1);
        assert_eq!(
            converted["messages"][0]["content"],
            serde_json::json!([
                {"type": "text", "text": "one"},
                {"type": "text", "text": "two"}
            ])
        );
    }

    #[test]
    fn converts_anthropic_responses_back_to_openai() {
        let converted = anthropic_to_openai_response(
            &serde_json::json!({
                "id": "msg_1",
                "model": "claude-upstream",
                "content": [
                    {"type": "text", "text": "hey"},
                    {"type": "tool_use", "id": "toolu_1", "name": "weather", "input": {"city": "sf"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 7, "output_tokens": 3}
            }),
            "fallback",
        );

        assert_eq!(converted["choices"][0]["message"]["content"], "hey");
        assert_eq!(converted["choices"][0]["finish_reason"], "tool_calls");
        assert_eq!(
            converted["choices"][0]["message"]["tool_calls"][0]["id"],
            "toolu_1"
        );
        assert_eq!(
            converted["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"],
            "{\"city\":\"sf\"}"
        );
        assert_eq!(converted["usage"]["prompt_tokens"], 7);
        assert_eq!(converted["usage"]["total_tokens"], 10);
        assert_eq!(converted["model"], "claude-upstream");
    }

    #[test]
    fn translates_anthropic_sse_frames_into_openai_chunks() {
        let mut adapter = AnthropicToOpenAiStream::new("claude-upstream");
        let mut frames = adapter.push(
            concat!(
                "event: message_start\n",
                "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_9\",\"model\":\"claude-real\"}}\n\n",
                "event: content_block_start\n",
                "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\"}}\n\n"
            )
            .as_bytes(),
        );

        let opening: serde_json::Value =
            serde_json::from_str(frames.remove(0).trim_start_matches("data: ").trim()).unwrap();
        assert_eq!(opening["choices"][0]["delta"]["role"], "assistant");
        assert_eq!(opening["model"], "claude-real");

        // An event split across two network chunks is held until it is whole.
        assert!(adapter
            .push(b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"ind")
            .is_empty());
        frames = adapter.push(b"ex\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"}}\n\n");
        let text: serde_json::Value =
            serde_json::from_str(frames.remove(0).trim_start_matches("data: ").trim()).unwrap();
        assert_eq!(text["choices"][0]["delta"]["content"], "hi");

        let mut closing = adapter.push(
            concat!(
                "event: message_delta\n",
                "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"max_tokens\"}}\n\n",
                "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"
            )
            .as_bytes(),
        );
        let final_chunk: serde_json::Value =
            serde_json::from_str(closing.remove(0).trim_start_matches("data: ").trim()).unwrap();
        assert_eq!(final_chunk["choices"][0]["finish_reason"], "length");
        assert_eq!(closing, vec!["data: [DONE]\n\n".to_string()]);
    }

    #[test]
    fn closes_a_stream_that_never_sent_message_stop() {
        let mut adapter = AnthropicToOpenAiStream::new("claude-upstream");
        let frames = adapter.push(
            b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"}}\n\n",
        );
        assert_eq!(frames.len(), 2);

        let closing = adapter.finish();
        assert_eq!(closing.len(), 2);
        assert!(closing[0].contains("\"finish_reason\":\"stop\""));
        assert_eq!(closing[1], "data: [DONE]\n\n");
    }

    #[test]
    fn converts_anthropic_image_and_document_inputs() {
        let converted = anthropic_to_openai(&serde_json::json!({
            "model": "claude-test",
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "Inspect these"},
                    {"type": "image", "source": {
                        "type": "base64", "media_type": "image/png", "data": "aW1hZ2U="
                    }},
                    {"type": "image", "source": {
                        "type": "url", "url": "https://example.com/image.png"
                    }},
                    {"type": "document", "title": "report.pdf", "source": {
                        "type": "base64", "media_type": "application/pdf", "data": "cGRm"
                    }}
                ]
            }]
        }));

        let content = converted["messages"][0]["content"].as_array().unwrap();
        assert_eq!(
            content[0],
            serde_json::json!({"type": "text", "text": "Inspect these"})
        );
        assert_eq!(
            content[1]["image_url"]["url"],
            "data:image/png;base64,aW1hZ2U="
        );
        assert_eq!(
            content[2]["image_url"]["url"],
            "https://example.com/image.png"
        );
        assert_eq!(content[3]["file"]["file_data"], "cGRm");
        assert_eq!(content[3]["file"]["filename"], "report.pdf");
    }

    #[test]
    fn collapses_anthropic_text_blocks_for_openai_compatibility() {
        let converted = anthropic_to_openai(&serde_json::json!({
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "Hello, "},
                    {"type": "text", "text": "world"}
                ]
            }]
        }));

        assert_eq!(converted["messages"][0]["content"], "Hello, world");
        assert!(converted.get("temperature").is_none());
    }

    #[test]
    fn preserves_tool_result_attachments_and_follow_up_text() {
        let converted = anthropic_to_openai(&serde_json::json!({
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "tool_result", "tool_use_id": "call_1", "content": [
                        {"type": "text", "text": "Screenshot captured."},
                        {"type": "image", "source": {
                            "type": "base64", "media_type": "image/jpeg", "data": "anBlZw=="
                        }}
                    ]},
                    {"type": "text", "text": "What is wrong?"}
                ]
            }]
        }));

        let messages = converted["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "tool");
        assert_eq!(messages[0]["tool_call_id"], "call_1");
        assert_eq!(messages[0]["content"], "Screenshot captured.");
        assert_eq!(messages[1]["role"], "user");
        let follow_up = messages[1]["content"].as_array().unwrap();
        assert_eq!(
            follow_up[1]["image_url"]["url"],
            "data:image/jpeg;base64,anBlZw=="
        );
        assert_eq!(follow_up[2]["text"], "What is wrong?");
    }

    #[test]
    fn historical_tool_calls_do_not_include_stream_indexes() {
        let converted = anthropic_to_openai(&serde_json::json!({
            "messages": [{
                "role": "assistant",
                "content": [{
                    "type": "tool_use",
                    "id": "call_1",
                    "name": "read_file",
                    "input": {"path": "a.txt"}
                }]
            }]
        }));

        let tool_call = &converted["messages"][0]["tool_calls"][0];
        assert_eq!(tool_call["id"], "call_1");
        assert!(tool_call.get("index").is_none());
    }
}
