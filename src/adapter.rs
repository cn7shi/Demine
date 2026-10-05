//! Codex rollout shapes are an implementation detail, not a stable protocol.
//! This module translates observable evidence without inferring root causes.
use serde::Serialize;
use serde_json::{Value, json};

#[derive(Debug, Clone, Serialize)]
pub struct Event {
    pub key: String,
    pub kind: String,
    pub title: String,
    pub input: String,
    pub output: String,
    pub status: String,
    pub timestamp: String,
    pub turn_id: Option<String>,
    pub call_id: Option<String>,
    pub stream: String,
    pub priority: i32,
}

pub struct Parsed {
    pub raw: String,
    pub record_type: String,
    pub event: Option<Event>,
    pub turn_id: Option<String>,
}

pub fn text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Array(items) => items.iter().map(text).collect::<Vec<_>>().join("\n"),
        Value::Object(map) => map
            .get("text")
            .map(text)
            .unwrap_or_else(|| value.to_string()),
        _ => value.to_string(),
    }
}

fn string(value: &Value, name: &str) -> String {
    value[name].as_str().unwrap_or_default().to_owned()
}

/// `None` deliberately excludes internal instructions/reasoning and bookkeeping.
/// Malformed input becomes visible evidence so progress can continue past it.
pub fn parse(raw: &str, line: u64, current_turn: Option<&str>) -> Option<Parsed> {
    let value: Value = match serde_json::from_str(raw) {
        Ok(value) => value,
        Err(_) => {
            return Some(Parsed {
                raw: raw.to_owned(),
                record_type: "malformed".into(),
                event: Some(Event {
                    key: format!("line:{line}"),
                    kind: "warning".into(),
                    title: "一条记录无法解析".into(),
                    input: String::new(),
                    output: "已保留原文；后续记录会继续采集。".into(),
                    status: "unknown".into(),
                    timestamp: String::new(),
                    turn_id: current_turn.map(str::to_owned),
                    call_id: None,
                    stream: "other".into(),
                    priority: 0,
                }),
                turn_id: None,
            });
        }
    };
    let record_type = string(&value, "type");
    let payload = &value["payload"];
    let subtype = string(payload, "type");
    let turn = payload["turn_id"].as_str().map(str::to_owned);
    if record_type == "turn_context" {
        return Some(Parsed {
            // Turn context may carry internal instructions. Keep only the pointer.
            raw: json!({"type":"turn_context","payload":{"turn_id":turn}}).to_string(),
            record_type,
            event: None,
            turn_id: turn,
        });
    }
    if matches!(record_type.as_str(), "token_usage_record" | "world_state")
        || (record_type == "response_item" && subtype == "reasoning")
        || (record_type == "event_msg"
            && matches!(
                subtype.as_str(),
                "token_count" | "thread_settings_applied" | "agent_reasoning"
            ))
    {
        return None;
    }
    if record_type == "session_meta" {
        return Some(Parsed {
            raw: json!({"type":"session_meta","timestamp":value["timestamp"],"payload":{
                "id":payload["id"],"cwd":payload["cwd"],"cli_version":payload["cli_version"]
            }})
            .to_string(),
            record_type,
            event: None,
            turn_id: None,
        });
    }

    let mut event = Event {
        key: format!("line:{line}"),
        kind: "unknown".into(),
        title: format!("未识别记录 · {record_type}/{subtype}"),
        input: String::new(),
        output: "原始记录已保留，当前适配器尚未解释此类型。".into(),
        status: "unknown".into(),
        timestamp: string(&value, "timestamp"),
        turn_id: turn.clone().or_else(|| current_turn.map(str::to_owned)),
        call_id: None,
        stream: "other".into(),
        priority: 0,
    };
    // Compaction can carry opaque internal history. Keep an allowlisted marker,
    // never the encrypted payload or internal message metadata.
    if record_type == "response_item" && subtype == "compaction" {
        event.kind = "context".into();
        event.title = "会话上下文发生压缩".into();
        event.output = "已保留压缩标记，内部压缩内容不采集。".into();
        event.status = "recorded".into();
        return Some(Parsed {
            raw: json!({"type":"response_item","timestamp":value["timestamp"],
                "payload":{"type":"compaction","id":payload["id"]}})
            .to_string(),
            record_type,
            event: Some(event),
            turn_id: turn,
        });
    }
    if record_type == "response_item" {
        event.stream = "response".into();
        event.priority = 1;
        match subtype.as_str() {
            "message" => {
                let role = string(payload, "role");
                if !matches!(role.as_str(), "user" | "assistant")
                    || payload["channel"] == "analysis"
                    || payload["phase"] == "analysis"
                {
                    return None;
                }
                let id = string(payload, "id");
                if !id.is_empty() {
                    event.key = format!("item:{id}");
                }
                event.kind = role.clone();
                event.title = if role == "user" {
                    "你的输入"
                } else {
                    "Agent 回复"
                }
                .into();
                event.output = text(&payload["content"]);
                event.status = "recorded".into();
                if role == "user" {
                    // Inputs may precede their new task_started marker. Do not
                    // assign them to the previous task merely by proximity.
                    event.turn_id = turn.clone();
                    if event
                        .output
                        .trim_start()
                        .starts_with("<environment_context>")
                        || event
                            .output
                            .trim_start()
                            .starts_with("<external_codex_apps_open_page>")
                    {
                        event.kind = "context".into();
                        event.title = "会话环境记录".into();
                    }
                }
            }
            "function_call" | "custom_tool_call" => {
                let id = string(payload, "call_id");
                if !id.is_empty() {
                    event.key = format!("call:{id}");
                    event.call_id = Some(id);
                }
                let name = string(payload, "name");
                // Code-mode envelopes and their nested executions are different layers.
                event.kind = if name == "functions.exec" || name == "exec" {
                    "transport"
                } else {
                    "tool"
                }
                .into();
                event.title = format!("工具请求 · {name}");
                event.input = text(payload.get("arguments").unwrap_or(&payload["input"]));
                event.output.clear();
                event.status = "started".into();
            }
            "function_call_output" | "custom_tool_call_output" => {
                let id = string(payload, "call_id");
                if !id.is_empty() {
                    event.key = format!("call:{id}");
                    event.call_id = Some(id);
                }
                event.kind = "tool".into();
                event.title = "工具返回（请求可能不在记录内）".into();
                event.output = text(&payload["output"]);
                event.status = "returned".into();
            }
            _ => {}
        }
    } else if record_type == "event_msg" {
        match subtype.as_str() {
            "task_started" | "task_complete" | "task_completed" | "turn_aborted" => {
                event.kind = "turn".into();
                event.title = match subtype.as_str() {
                    "task_started" => "本轮开始",
                    "turn_aborted" => "本轮中断",
                    _ => "本轮结束",
                }
                .into();
                event.output = "回合状态只描述执行过程，不代表问题已验证解决。".into();
                event.status = if subtype == "task_started" {
                    "started"
                } else if subtype == "turn_aborted" {
                    "interrupted"
                } else {
                    "completed"
                }
                .into();
            }
            "user_message" | "agent_message" => {
                event.stream = "legacy".into();
                event.kind = if subtype == "user_message" {
                    "user"
                } else {
                    "assistant"
                }
                .into();
                event.title = if subtype == "user_message" {
                    "你的输入"
                } else {
                    "Agent 回复"
                }
                .into();
                event.output = text(&payload["message"]);
                event.status = "recorded".into();
                if subtype == "user_message" {
                    event.turn_id = turn.clone();
                }
            }
            "item_completed" | "item_started" => {
                let item = &payload["item"];
                let kind = string(item, "type");
                if matches!(kind.as_str(), "Reasoning" | "reasoning") {
                    return None;
                }
                let id = string(item, "id");
                if !id.is_empty() {
                    event.key = format!("item:{id}");
                }
                event.stream = "item".into();
                event.priority = 2;
                event.status = if subtype == "item_started" {
                    "started"
                } else {
                    "completed"
                }
                .into();
                event.input.clear();
                event.output.clear();
                match kind.as_str() {
                    "UserMessage" | "userMessage" => {
                        event.kind = "user".into();
                        event.title = "你的输入".into();
                        event.output = text(&item["content"]);
                        event.status = "recorded".into();
                        event.turn_id = turn.clone();
                    }
                    "AgentMessage" | "agentMessage" => {
                        if item["phase"] == "analysis" {
                            return None;
                        }
                        event.kind = "assistant".into();
                        event.title = "Agent 回复".into();
                        event.output = text(item.get("content").unwrap_or(&item["text"]));
                        event.status = "recorded".into();
                    }
                    "CommandExecution" | "commandExecution" => {
                        event.kind = "command".into();
                        event.title = "执行命令".into();
                        event.input = text(&item["command"]);
                        event.output = text(
                            item.get("aggregated_output")
                                .or_else(|| item.get("aggregatedOutput"))
                                .unwrap_or(&item["stdout"]),
                        );
                        if let Some(code) = item
                            .get("exit_code")
                            .or_else(|| item.get("exitCode"))
                            .and_then(Value::as_i64)
                        {
                            event.status = if code == 0 { "succeeded" } else { "failed" }.into();
                            event.title = format!("执行命令 · 退出码 {code}");
                        }
                    }
                    "McpToolCall" | "mcpToolCall" => {
                        event.kind = "tool".into();
                        event.title = format!(
                            "工具调用 · {} / {}",
                            string(item, "server"),
                            string(item, "tool")
                        );
                        event.input = text(&item["arguments"]);
                        event.output = text(item.get("result").unwrap_or(&item["error"]));
                        if item["result"]["isError"] == true
                            || !item["error"].is_null()
                            || item["status"] == "Failed"
                            || item["status"] == "failed"
                        {
                            event.status = "failed".into();
                        }
                    }
                    "FileChange" | "fileChange" => {
                        event.kind = "file".into();
                        event.title = "文件改动".into();
                        event.output = text(&item["changes"]);
                    }
                    "Extension" => {
                        event.kind = "tool".into();
                        event.title = format!("扩展操作 · {}", string(item, "kind"));
                        event.input = text(&item["query"]);
                        event.output = text(&item["results"]);
                    }
                    _ => {
                        event.title = format!("未识别操作 · {kind}");
                        event.output = "可展开查看原始证据。".into();
                    }
                }
                // A launch/apply failure may have no exit code. Its explicit
                // failure status must not be replaced with generic completion.
                if matches!(event.kind.as_str(), "command" | "file" | "tool")
                    && item["status"]
                        .as_str()
                        .is_some_and(|status| status.eq_ignore_ascii_case("failed"))
                {
                    event.status = "failed".into();
                }
            }
            _ => {}
        }
    } else if record_type == "compacted" {
        event.kind = "context".into();
        event.title = "会话上下文发生压缩".into();
        event.output = "此前采集的执行证据仍保留；压缩不代表这些问题已经解决。".into();
        // Compacted replacement history may contain instructions and reasoning.
        return Some(Parsed {
            raw: json!({"type":"compacted","timestamp":value["timestamp"]}).to_string(),
            record_type,
            event: Some(event),
            turn_id: turn,
        });
    }
    Some(Parsed {
        raw: raw.to_owned(),
        record_type,
        event: Some(event),
        turn_id: turn,
    })
}
