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
    #[serde(default)]
    pub path: Option<String>,
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
    pub cwd: Option<String>,
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
    let rows = session_rows(value)?;
    let allowed: HashSet<_> = workspace.session_ids.iter().collect();
    let mut result = Vec::new();
    for row in rows {
        if allowed.contains(&row.session_id)
            && row.parent_session_id.is_none()
            && row.origin.as_deref() != Some("subagent")
        {
            result.push(row);
        }
    }
    Ok(result)
}

/// Parse and identity-check the official session catalog once; membership
/// filtering is layered on by the caller (workspace-owned or unassigned).
pub(crate) fn session_rows(value: Value) -> Result<Vec<SessionRow>> {
    let rows: Vec<SessionRow> = serde_json::from_value(
        value
            .get("items")
            .cloned()
            .ok_or_else(|| RemoteError::invalid("Missing native session catalog"))?,
    )
    .map_err(|_| RemoteError::invalid("Invalid native session catalog"))?;
    let mut ids = HashSet::new();
    for row in &rows {
        if row.session_id.is_empty() || !ids.insert(row.session_id.clone()) {
            return Err(RemoteError::invalid("Duplicate or empty native session ID"));
        }
    }
    Ok(rows)
}

/// Reserved virtual workspace for true unassigned Host sessions (v3 only).
/// It never appears in the official baseline; membership is derived from the
/// session catalog's authoritative cwd, restricted to the exact Papers path.
pub(crate) const UNASSIGNED_WORKSPACE_ID: &str = "agentero:unassigned";

/// Windows Hosts canonicalize separators and case; compare drive paths with
/// normalized separators and case folding. Unix paths stay case-sensitive.
pub(crate) fn same_host_path(left: &str, right: &str) -> bool {
    fn normalize(path: &str) -> String {
        let trimmed = path.trim_end_matches(['/', '\\']);
        let windows = trimmed.len() >= 2
            && trimmed.as_bytes()[0].is_ascii_alphabetic()
            && trimmed.as_bytes()[1] == b':';
        if windows {
            trimmed.replace('\\', "/").to_ascii_lowercase()
        } else {
            trimmed.to_owned()
        }
    }
    normalize(left) == normalize(right)
}

pub(crate) fn unassigned_sessions(
    rows: Vec<SessionRow>,
    owned: &HashSet<String>,
    papers_cwd: &str,
) -> Vec<SessionRow> {
    rows.into_iter()
        .filter(|row| {
            row.parent_session_id.is_none()
                && row.origin.as_deref() != Some("subagent")
                && !owned.contains(&row.session_id)
                && row
                    .cwd
                    .as_deref()
                    .is_some_and(|cwd| same_host_path(cwd, papers_cwd))
        })
        .collect()
}

/// Project the official contextPressure projection into bounded token counts.
/// Installed dsh-token-meter prefers projectedTokens over pressureTokens;
/// older/ACP hosts emit {used,size}. A window size is required to display.
pub(crate) fn context_usage(snapshot: &Value) -> Option<ContextUsage> {
    let pressure = snapshot.pointer("/projections/values/contextPressure")?;
    let used = ["projectedTokens", "pressureTokens", "used"]
        .into_iter()
        .find_map(|key| pressure.get(key).and_then(Value::as_u64))?;
    let size = ["contextWindow", "size"]
        .into_iter()
        .find_map(|key| pressure.get(key).and_then(Value::as_u64))
        .filter(|size| *size > 0)?;
    if used > super::protocol::MAX_SAFE_INTEGER || size > super::protocol::MAX_SAFE_INTEGER {
        return None;
    }
    Some(ContextUsage { used, size })
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelCatalog {
    default: RemoteModelSelection,
    routable_providers: Option<Vec<String>>,
    groups: Vec<RemoteModelProviderGroup>,
    #[serde(default)]
    failures: Vec<RemoteModelProviderFailure>,
}

fn valid_selection(selection: &RemoteModelSelection) -> bool {
    !selection.provider.trim().is_empty() && !selection.model.trim().is_empty()
}

pub(crate) fn session_models(row: &SessionRow, value: Value) -> Result<RemoteSessionModels> {
    models(row.projections.pointer("/values/modelSelection"), value)
}

/// Host-global catalog projection: no session row exists yet, so the current
/// selection is the Host default (official sessionModels without a cache hit).
pub(crate) fn host_models(value: Value) -> Result<RemoteSessionModels> {
    models(None, value)
}

fn models(projected: Option<&Value>, value: Value) -> Result<RemoteSessionModels> {
    let catalog: ModelCatalog = serde_json::from_value(value)
        .map_err(|_| RemoteError::invalid("Invalid native model catalog"))?;
    if !valid_selection(&catalog.default) {
        return Err(RemoteError::invalid("Invalid native default model"));
    }
    let current = projected
        .and_then(projected_selection)
        .unwrap_or(catalog.default);
    let routable = catalog.routable_providers.as_ref().map_or_else(
        || !catalog.groups.is_empty(),
        |providers| providers.contains(&current.provider),
    );
    Ok(RemoteSessionModels {
        current,
        routable,
        groups: catalog.groups,
        failures: catalog.failures,
    })
}

fn projected_selection(projection: &Value) -> Option<RemoteModelSelection> {
    [
        projection.get("next"),
        projection.get("lastUsed"),
        Some(projection),
    ]
    .into_iter()
    .flatten()
    .find_map(|candidate| {
        let selection: RemoteModelSelection = serde_json::from_value(candidate.clone()).ok()?;
        valid_selection(&selection).then_some(selection)
    })
}

pub(crate) fn selected_model(value: &Value) -> Result<RemoteModelSelection> {
    let selected = value
        .get("selected")
        .cloned()
        .and_then(|selected| serde_json::from_value(selected).ok())
        .filter(valid_selection)
        .ok_or_else(|| RemoteError::invalid("Invalid native model selection"))?;
    Ok(selected)
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

/// Absolute Host-side directory for workspace/create. The Host owns existence
/// and permission checks; this only bounds the opaque path reference.
pub(crate) fn valid_workspace_path(path: &str) -> bool {
    let trimmed = path.trim();
    let drive = trimmed.len() >= 3
        && trimmed.as_bytes()[0].is_ascii_alphabetic()
        && trimmed.as_bytes()[1] == b':'
        && matches!(trimmed.as_bytes()[2], b'/' | b'\\');
    let absolute = trimmed.starts_with('/') || drive;
    let root = matches!(trimmed, "/" | "\\") || (trimmed.len() == 3 && drive);
    absolute && !root && path.len() <= 1024 && !path.chars().any(|c| c == '\0' || c.is_control())
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
/// reasoning blocks ride along with their assistant message. Real Hosts emit
/// extra event types and message variants: an unparseable known shape is
/// skipped, never fatal to the whole read (pagination integrity stays strict
/// in `records`).
pub(crate) fn events(session: &SessionRef, native: &Value) -> Result<Vec<RemoteEvent>> {
    let seq = integer(native, "seq")?;
    let kind = string(native, "type")?;
    if kind.starts_with("chunkrow/") {
        return chunk_events(session, native);
    }
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
            // Official isHumanUserMessage reads `data.message ?? data`, so
            // both nested and flat shapes carry the source.
            let message = data
                .get("message")
                .filter(|m| m.is_object())
                .unwrap_or(data);
            if message.pointer("/source/kind").and_then(Value::as_str) != Some("user") {
                return Ok(Vec::new());
            }
            let Some(content) = message_content(data) else {
                return Ok(Vec::new());
            };
            let text = content_text(content, "text");
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
            let Some(content) = message_content(data) else {
                return Ok(Vec::new());
            };
            let text = content_text(content, "text");
            let reasoning = content_text(content, "reasoning");
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
            let Some(call_id) = data
                .pointer("/message/source/callId")
                .and_then(Value::as_str)
                .or_else(|| data.get("callId").and_then(Value::as_str))
                .or_else(|| data.get("id").and_then(Value::as_str))
            else {
                return Ok(Vec::new());
            };
            let call_id = call_id.to_owned();
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
        // Replacement surfaces and unknown event types have no faithful
        // equivalent in the shared snapshot contract; final messages remain intact.
        _ => return Ok(Vec::new()),
    };
    Ok(vec![make(format!("seq:{seq}"), payload)])
}

/// One packed streaming row (`chunkrow/text-chunks` / `chunkrow/reasoning-chunks`)
/// becomes ONE AssistantChunk whose delta concatenates the row's texts.
/// Tool-call argument chunks are skipped: the durable tool/call row carries the
/// final arguments. Source IDs are row-stable so history replay deduplicates.
pub(crate) fn chunk_events(session: &SessionRef, native: &Value) -> Result<Vec<RemoteEvent>> {
    let Ok(seq) = integer(native, "seq") else {
        return Ok(Vec::new());
    };
    let kind = native.get("type").and_then(Value::as_str).unwrap_or("");
    let data = native.get("data").unwrap_or(&Value::Null);
    let joined = match kind {
        "chunkrow/text-chunks" | "chunkrow/reasoning-chunks" => joined_strings(data.get("texts")),
        _ => return Ok(Vec::new()),
    };
    if joined.is_empty() {
        return Ok(Vec::new());
    }
    let bounded = bounded_text(&joined);
    let payload = if kind == "chunkrow/text-chunks" {
        EventPayload::AssistantChunk {
            text_delta: Some(bounded),
            reasoning_delta: None,
        }
    } else {
        EventPayload::AssistantChunk {
            text_delta: None,
            reasoning_delta: Some(bounded),
        }
    };
    Ok(vec![RemoteEvent {
        session: session.clone(),
        source_stream_id: source_stream(session),
        source_id: format!("chunk:{seq}"),
        cursor: Some(cursor(session, seq as i64)),
        payload,
    }])
}

/// Project one live single-chunk payload (`{type:'text-delta'|'reasoning-delta',
/// text}`) into an AssistantChunk. Non-text chunk kinds (block-start, finish,
/// usage, tool-call deltas) surface nothing here.
pub(crate) fn live_chunk_event(
    session: &SessionRef,
    attempt_id: &str,
    index: u64,
    chunk: &Value,
) -> Option<RemoteEvent> {
    let text = chunk.get("text").and_then(Value::as_str)?;
    if text.is_empty() {
        return None;
    }
    let payload = match chunk.get("type").and_then(Value::as_str)? {
        "text-delta" => EventPayload::AssistantChunk {
            text_delta: Some(bounded_text(text)),
            reasoning_delta: None,
        },
        "reasoning-delta" => EventPayload::AssistantChunk {
            text_delta: None,
            reasoning_delta: Some(bounded_text(text)),
        },
        _ => return None,
    };
    Some(stream_event(session, attempt_id, index, payload))
}

/// Expand `snapshot.assistantStream.activeAttempt.stream` (raw `{type:'chunk'}`
/// records and compact `*-chunks` rows, official expandAssistantStreamRecords)
/// into AssistantChunk events appended AFTER the durable history. Capped at
/// `nextIndex` expanded members and bounded like a history page.
pub(crate) fn stream_events(session: &SessionRef, snapshot: &Value) -> Result<Vec<RemoteEvent>> {
    const STREAM_LIMIT: usize = 4 * 1024 * 1024;
    let Some(attempt) = snapshot.pointer("/assistantStream/activeAttempt") else {
        return Ok(Vec::new());
    };
    let (Some(attempt_id), Some(next_index), Some(stream)) = (
        attempt.get("attemptId").and_then(Value::as_str),
        attempt.get("nextIndex").and_then(Value::as_u64),
        attempt.get("stream").and_then(Value::as_array),
    ) else {
        return Ok(Vec::new());
    };
    let mut events = Vec::new();
    let mut index: u64 = 0;
    let mut bytes = 0usize;
    for record in stream {
        if index >= next_index || bytes > STREAM_LIMIT {
            break;
        }
        let Some(kind) = record.get("type").and_then(Value::as_str) else {
            continue;
        };
        // Each expanded member consumes one attempt index, even when it does
        // not surface (block-start, tool-call deltas, empty text).
        let members: Vec<(Option<String>, Option<String>)> = match kind {
            "chunk" => {
                let chunk = record.get("chunk").unwrap_or(&Value::Null);
                let text = chunk.get("text").and_then(Value::as_str);
                match chunk.get("type").and_then(Value::as_str) {
                    Some("text-delta") => vec![(text.map(str::to_owned), None)],
                    Some("reasoning-delta") => vec![(None, text.map(str::to_owned))],
                    _ => vec![(None, None)],
                }
            }
            "text-chunks" => strings(record.get("texts"))
                .into_iter()
                .map(|text| (Some(text), None))
                .collect(),
            "reasoning-chunks" => strings(record.get("texts"))
                .into_iter()
                .map(|text| (None, Some(text)))
                .collect(),
            "tool-call-chunks" => strings(record.get("args"))
                .into_iter()
                .map(|_| (None, None))
                .collect(),
            _ => continue,
        };
        for (text_delta, reasoning_delta) in members {
            if index >= next_index || bytes > STREAM_LIMIT {
                break;
            }
            let current = index;
            index += 1;
            let payload = match (text_delta, reasoning_delta) {
                (Some(text), None) if !text.is_empty() => EventPayload::AssistantChunk {
                    text_delta: Some(bounded_text(&text)),
                    reasoning_delta: None,
                },
                (None, Some(reasoning)) if !reasoning.is_empty() => EventPayload::AssistantChunk {
                    text_delta: None,
                    reasoning_delta: Some(bounded_text(&reasoning)),
                },
                _ => continue,
            };
            if let EventPayload::AssistantChunk {
                text_delta,
                reasoning_delta,
            } = &payload
            {
                bytes += text_delta.as_deref().map_or(0, str::len)
                    + reasoning_delta.as_deref().map_or(0, str::len);
            }
            events.push(stream_event(session, attempt_id, current, payload));
        }
    }
    Ok(events)
}

fn stream_event(
    session: &SessionRef,
    attempt_id: &str,
    index: u64,
    payload: EventPayload,
) -> RemoteEvent {
    RemoteEvent {
        session: session.clone(),
        source_stream_id: source_stream(session),
        source_id: format!("stream:{attempt_id}:{index}"),
        // Transient chunks have no durable sequence; the snapshot cursor stays
        // authoritative for subscription recovery.
        cursor: None,
        payload,
    }
}

fn strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn joined_strings(value: Option<&Value>) -> String {
    strings(value).join("")
}

/// Official messageText/messageReasoning read `data.message ?? data`; accept
/// both nested and flat content arrays. Missing content is a skip, not an error.
fn message_content(data: &Value) -> Option<&Vec<Value>> {
    data.pointer("/message/content")
        .or_else(|| data.get("content"))
        .and_then(Value::as_array)
}
fn content_text(content: &[Value], block_type: &str) -> String {
    content
        .iter()
        .filter(|part| part.get("type").and_then(Value::as_str) == Some(block_type))
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
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
