//! Wire projections from upstream HarnessAlphaClient/remote-session-tools, 11064af.
use super::{
    gateway::{integer, string},
    RemoteError, Result,
};
use crate::features::remote_agent::*;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashSet;

pub(crate) const BACKEND: &str = "harness";
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceRow {
    pub workspace_id: String,
    pub title: String,
    pub session_ids: Vec<String>,
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionRow {
    pub session_id: String,
    pub running: bool,
    pub title: Option<String>,
    pub parent_session_id: Option<String>,
    pub origin: Option<String>,
    #[serde(default)]
    pub projections: Value,
}
pub(crate) fn workspaces(baseline: &Value) -> Result<Vec<WorkspaceRow>> {
    if baseline.get("type").and_then(Value::as_str) != Some("baseline") {
        return Err(RemoteError::invalid("Invalid native workspace baseline"));
    }
    let rows: Vec<WorkspaceRow> = serde_json::from_value(
        baseline
            .pointer("/value/items")
            .cloned()
            .ok_or_else(|| RemoteError::invalid("Missing native workspace catalog"))?,
    )
    .map_err(|_| RemoteError::invalid("Invalid native workspace catalog"))?;
    if rows.len() > 4096 {
        return Err(RemoteError::invalid(
            "Native workspace catalog exceeds limit",
        ));
    }
    let mut ids = HashSet::new();
    for row in &rows {
        if row.workspace_id.is_empty() || !ids.insert(&row.workspace_id) {
            return Err(RemoteError::invalid(
                "Duplicate or empty native workspace ID",
            ));
        }
    }
    Ok(rows)
}
pub(crate) fn sessions(value: Value, workspace: &WorkspaceRow) -> Result<Vec<SessionRow>> {
    let rows: Vec<SessionRow> = serde_json::from_value(
        value
            .get("items")
            .cloned()
            .ok_or_else(|| RemoteError::invalid("Missing native session catalog"))?,
    )
    .map_err(|_| RemoteError::invalid("Invalid native session catalog"))?;
    let allowed: HashSet<_> = workspace.session_ids.iter().collect();
    let mut ids = HashSet::new();
    let mut result = Vec::new();
    for row in rows {
        if row.session_id.is_empty() || !ids.insert(row.session_id.clone()) {
            return Err(RemoteError::invalid("Duplicate or empty native session ID"));
        }
        if allowed.contains(&row.session_id)
            && row.parent_session_id.is_none()
            && row.origin.as_deref() != Some("subagent")
        {
            result.push(row);
        }
    }
    Ok(result)
}
pub(crate) fn ensure_backend(workspace: &WorkspaceRef) -> Result<()> {
    if workspace.backend_id.0 != BACKEND {
        return Err(RemoteError::new(
            "FEATURE_NOT_SUPPORTED",
            "Only native Harness sessions are mapped by this adapter",
        ));
    }
    Ok(())
}
pub(crate) fn address(session: &SessionRef) -> Value {
    json!({"kind":"session","sessionId":session.session_id.0})
}
pub(crate) fn source_stream(session: &SessionRef) -> String {
    format!(
        "harness:{}",
        json!([
            session.workspace.host.connection.provider_id.0,
            session.workspace.host.connection.connection_id.0,
            session.workspace.host.host_id.0,
            session.workspace.workspace_id.0,
            session.session_id.0
        ])
    )
}
pub(crate) fn cursor(session: &SessionRef, seq: i64) -> EventCursor {
    EventCursor {
        stream_id: source_stream(session),
        value: seq.to_string(),
    }
}
pub(crate) fn parse_cursor(session: &SessionRef, cursor: &EventCursor) -> Result<i64> {
    if cursor.stream_id != source_stream(session) {
        return Err(RemoteError::invalid(
            "Native cursor belongs to a different session",
        ));
    }
    let seq: i64 = cursor
        .value
        .parse()
        .map_err(|_| RemoteError::invalid("Invalid native history cursor"))?;
    if seq < -1 || seq > super::protocol::MAX_SAFE_INTEGER as i64 || seq.to_string() != cursor.value
    {
        return Err(RemoteError::invalid("Invalid native history cursor"));
    }
    Ok(seq)
}
pub(crate) fn through(value: &Value) -> Result<i64> {
    value
        .get("cursor")
        .and_then(Value::as_i64)
        .filter(|s| *s >= -1 && *s <= super::protocol::MAX_SAFE_INTEGER as i64)
        .ok_or_else(|| RemoteError::invalid("Invalid native snapshot cursor"))
}
pub(crate) fn records(page: &Value, through: i64) -> Result<Vec<Value>> {
    if page.get("hasMore").and_then(Value::as_bool).is_none() {
        return Err(RemoteError::invalid(
            "Missing native history pagination state",
        ));
    }
    let rows = page
        .get("records")
        .and_then(Value::as_array)
        .ok_or_else(|| RemoteError::invalid("Missing native history records"))?;
    let mut previous = None;
    for row in rows {
        if !matches!(
            row.get("type").and_then(Value::as_str),
            Some("event" | "chunks")
        ) {
            return Err(RemoteError::invalid("Invalid native history record type"));
        }
        let event = row
            .get("event")
            .ok_or_else(|| RemoteError::invalid("Missing native event"))?;
        let seq = integer(event, "seq")?;
        string(event, "type")?;
        if seq as i64 > through || previous.is_some_and(|before| seq <= before) {
            return Err(RemoteError::invalid(
                "Invalid or nonmonotonic native event sequence",
            ));
        }
        previous = Some(seq);
    }
    Ok(rows.clone())
}
/// Project one native event into zero or more shared events. Tool calls and
/// results keep the native correlation ID so clients fold them into one row;
/// reasoning blocks ride along with their assistant message.
pub(crate) fn events(session: &SessionRef, native: &Value) -> Result<Vec<RemoteEvent>> {
    let seq = integer(native, "seq")?;
    let kind = string(native, "type")?;
    let data = native.get("data").unwrap_or(&Value::Null);
    let make = |source_id: String, payload: EventPayload| RemoteEvent {
        session: session.clone(),
        source_stream_id: source_stream(session),
        source_id,
        cursor: Some(cursor(session, seq as i64)),
        payload,
    };
    let payload = match kind {
        "user/message" => {
            // Harness also records model-facing injected context (runtime
            // snapshots, skill bodies, notices) as user/message. Only the
            // explicit human source belongs in the conversation transcript.
            if data.pointer("/message/source/kind").and_then(Value::as_str) != Some("user") {
                return Ok(Vec::new());
            }
            let content = message_content(data)?;
            let text = content_text(content, "text")?;
            if text.is_empty() {
                return Ok(Vec::new());
            }
            EventPayload::Message {
                role: "user".into(),
                text,
                reasoning: None,
            }
        }
        "assistant/message" => {
            let content = message_content(data)?;
            let text = content_text(content, "text")?;
            let reasoning = content_text(content, "reasoning")?;
            if text.is_empty() && reasoning.is_empty() {
                return Ok(Vec::new());
            }
            EventPayload::Message {
                role: "assistant".into(),
                text,
                reasoning: (!reasoning.is_empty()).then_some(reasoning),
            }
        }
        "tool/call" | "tool/result" => {
            let call_id = data
                .pointer("/message/source/callId")
                .and_then(Value::as_str)
                .or_else(|| data.get("callId").and_then(Value::as_str))
                .or_else(|| data.get("id").and_then(Value::as_str))
                .ok_or_else(|| RemoteError::invalid("Missing native tool call ID"))?
                .to_owned();
            let name = data
                .get("name")
                .and_then(Value::as_str)
                .or_else(|| data.get("toolName").and_then(Value::as_str))
                .unwrap_or("Tool")
                .to_owned();
            let finished = kind == "tool/result";
            let failed = finished && tool_result_failed(data);
            let arguments = data
                .get("arguments")
                .and_then(Value::as_str)
                .or_else(|| data.get("argumentsDelta").and_then(Value::as_str))
                .map(bounded_text);
            let output = finished.then(|| tool_output_text(data)).flatten();
            return Ok(vec![make(
                format!("tool:{call_id}"),
                EventPayload::Tool {
                    call_id,
                    name,
                    state: if failed {
                        "failed"
                    } else if finished {
                        "finished"
                    } else {
                        "running"
                    }
                    .into(),
                    arguments,
                    output,
                },
            )]);
        }
        "turn/start" => EventPayload::TaskStatus {
            status: TaskStatus::Running,
        },
        "turn/end" => EventPayload::TaskStatus {
            status: match data
                .get("reason")
                .and_then(Value::as_str)
                .or_else(|| data.pointer("/reason/kind").and_then(Value::as_str))
            {
                Some("error" | "failed") => TaskStatus::Failed,
                Some("cancelled" | "canceled") => TaskStatus::Cancelled,
                _ => TaskStatus::Idle,
            },
        },
        // Streaming chunks and replacement surfaces have no faithful equivalent
        // in the shared snapshot contract; final native messages remain intact.
        _ => return Ok(Vec::new()),
    };
    Ok(vec![make(format!("seq:{seq}"), payload)])
}
fn message_content(data: &Value) -> Result<&Vec<Value>> {
    data.pointer("/message/content")
        .or_else(|| data.get("content"))
        .and_then(Value::as_array)
        .ok_or_else(|| RemoteError::invalid("Missing native message content"))
}
fn content_text(content: &[Value], block_type: &str) -> Result<String> {
    content
        .iter()
        .filter(|part| part.get("type").and_then(Value::as_str) == Some(block_type))
        .map(|part| {
            part.get("text")
                .and_then(Value::as_str)
                .ok_or_else(|| RemoteError::invalid("Invalid native text content"))
        })
        .collect::<Result<Vec<_>>>()
        .map(|parts| parts.join("\n"))
}
/// Tool output may live in message.content text blocks or nested tool-result
/// blocks; join whichever the Host persisted, bounded for the snapshot surface.
fn tool_output_text(data: &Value) -> Option<String> {
    let content = data
        .pointer("/message/content")
        .or_else(|| data.get("content"))
        .and_then(Value::as_array)?;
    let mut parts = Vec::new();
    for block in content {
        let block_type = block.get("type").and_then(Value::as_str);
        if block_type == Some("tool-result") {
            if let Some(nested) = block.get("content").and_then(Value::as_array) {
                for inner in nested {
                    if inner.get("type").and_then(Value::as_str) == Some("text") {
                        if let Some(text) = inner.get("text").and_then(Value::as_str) {
                            parts.push(text);
                        }
                    }
                }
            }
        } else if block_type == Some("text") {
            if let Some(text) = block.get("text").and_then(Value::as_str) {
                parts.push(text);
            }
        }
    }
    let joined = parts.join("\n");
    (!joined.is_empty()).then(|| bounded_text(&joined))
}
fn tool_result_failed(data: &Value) -> bool {
    data.get("isError").and_then(Value::as_bool) == Some(true)
        || data.get("error").is_some_and(|error| !error.is_null())
        || data.pointer("/message/isError").and_then(Value::as_bool) == Some(true)
}
/// Keep individual tool payloads bounded like the official clients (64 KB).
fn bounded_text(text: &str) -> String {
    const LIMIT: usize = 64 * 1024;
    if text.len() <= LIMIT {
        return text.to_owned();
    }
    let mut end = LIMIT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n…", &text[..end])
}
pub(crate) fn status(
    row: &SessionRow,
    records: &[Value],
    projection: Option<&Value>,
) -> Result<TaskStatus> {
    let mut status = if row.running {
        TaskStatus::Running
    } else {
        TaskStatus::Idle
    };
    for record in records {
        let native = &record["event"];
        match string(native, "type")? {
            "turn/start" => status = TaskStatus::Running,
            "turn/end" => {
                status = match native
                    .pointer("/data/reason")
                    .and_then(Value::as_str)
                    .or_else(|| native.pointer("/data/reason/kind").and_then(Value::as_str))
                {
                    Some("error" | "failed") => TaskStatus::Failed,
                    Some("cancelled" | "canceled") => TaskStatus::Cancelled,
                    _ => TaskStatus::Idle,
                }
            }
            _ => {}
        }
    }
    let values = projection
        .and_then(|p| p.get("values"))
        .or_else(|| row.projections.get("values"));
    if values
        .and_then(|v| v.pointer("/inbox/next-turn"))
        .and_then(Value::as_array)
        .is_some_and(|v| !v.is_empty())
    {
        status = TaskStatus::Running;
    }
    Ok(status)
}
pub(crate) fn questions(value: &Value) -> Result<Vec<Question>> {
    let rows = value
        .as_array()
        .filter(|v| v.len() <= 64)
        .ok_or_else(|| RemoteError::invalid("Invalid native questions"))?;
    let mut ids = HashSet::new();
    let mut out = Vec::new();
    for row in rows {
        let id = string(row, "id")?.to_owned();
        if !ids.insert(id.clone()) {
            return Err(RemoteError::invalid("Duplicate native question ID"));
        }
        let options = row
            .get("options")
            .and_then(Value::as_array)
            .map(|options| {
                options
                    .iter()
                    .map(|option| string(option, "label").map(str::to_owned))
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()?
            .unwrap_or_default();
        out.push(Question {
            id,
            prompt: string(row, "question")?.to_owned(),
            options,
            multiple: row
                .get("multiSelect")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            allows_custom: true,
        });
    }
    Ok(out)
}
