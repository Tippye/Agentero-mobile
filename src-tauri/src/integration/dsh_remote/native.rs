//! Native Harness adapter. Wire mappings follow upstream 11064af, never ApiProxy.
#[cfg(test)]
#[path = "native_tests.rs"]
mod tests;
use super::{
    gateway::{call, id, integer, string, GatewayStream, WireRpc, RPC_BUDGET},
    native_model as model,
    rpc::CallSemantics,
    RemoteError, Result,
};
use crate::features::remote_agent::*;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs::File,
    io::{Read, Seek, SeekFrom},
    sync::{Arc, Mutex as StdMutex},
    time::Duration,
};
use tokio::{
    sync::{broadcast, Mutex},
    time::Instant,
};

pub(crate) const UPLOAD_LIMIT: u64 = 20 * 1024 * 1024;
/// A trusted native file-picker/credential owner grants already-open file handles.
/// The wire client's local_path is only an opaque lookup key, never ambient read authority.
#[derive(Default)]
pub struct UploadGrants {
    files: StdMutex<HashMap<String, File>>,
}
impl UploadGrants {
    pub fn grant(&self, local_path: String, file: File) -> Result<()> {
        let metadata = file
            .metadata()
            .map_err(|_| RemoteError::invalid("Cannot inspect granted upload file"))?;
        if local_path.is_empty() || !metadata.is_file() || metadata.len() > UPLOAD_LIMIT {
            return Err(RemoteError::invalid(
                "Upload grant must be a regular file of at most 20 MiB",
            ));
        }
        let mut files = self
            .files
            .lock()
            .map_err(|_| RemoteError::invalid("Upload grants unavailable"))?;
        if files.len() >= 64 {
            return Err(RemoteError::new(
                "RATE_LIMITED",
                "Too many pending upload grants",
            ));
        }
        files.insert(local_path, file);
        Ok(())
    }
    fn take(&self, path: &str) -> Result<File> {
        self.files
            .lock()
            .map_err(|_| RemoteError::invalid("Upload grants unavailable"))?
            .remove(path)
            .ok_or_else(|| {
                RemoteError::new("PERMISSION_DENIED", "This file has no native upload grant")
            })
    }
}
struct PendingInteraction {
    session_id: String,
    event_id: Option<String>,
    client_id: Option<String>,
    questions: Option<Vec<Question>>,
    description: String,
    call_id: Option<String>,
    deadline: Option<Instant>,
    _claim: Option<GatewayStream>,
}
impl PendingInteraction {
    fn event(&self, session: &SessionRef, request_id: &str) -> RemoteEvent {
        let payload = match &self.questions {
            Some(questions) => EventPayload::QuestionRequest {
                request_id: request_id.into(),
                questions: questions.clone(),
            },
            None => EventPayload::PermissionRequest {
                request_id: request_id.into(),
                description: self.description.clone(),
                options: vec!["allowed-once".into(), "rejected".into()],
            },
        };
        RemoteEvent {
            session: session.clone(),
            source_stream_id: format!("{}:interactions", model::source_stream(session)),
            source_id: request_id.into(),
            cursor: None,
            payload,
        }
    }
}
#[derive(Default)]
struct Interactions {
    client_id: Option<String>,
    pending: HashMap<String, PendingInteraction>,
    consumed: HashSet<String>,
    failed: bool,
}

pub(crate) struct NativeHost {
    pub wire: Arc<dyn WireRpc>,
    pub capabilities: HashSet<String>,
    /// Host home directory advertised by the official ready frame, if any.
    pub home: Option<String>,
    interactions: Arc<Mutex<Interactions>>,
    interaction_events: broadcast::Sender<(String, String)>,
    grants: Arc<UploadGrants>,
    event_task: StdMutex<Option<tokio::task::JoinHandle<()>>>,
}
impl NativeHost {
    pub async fn start(
        wire: Arc<dyn WireRpc>,
        capabilities: Vec<String>,
        grants: Arc<UploadGrants>,
    ) -> Result<Arc<Self>> {
        let capabilities: HashSet<_> = capabilities.into_iter().collect();
        if capabilities.contains("harness.remote.v1") == capabilities.contains("harness.remote.v3")
        {
            return Err(RemoteError::new(
                "FEATURE_NOT_SUPPORTED",
                "Exactly one native Harness Session format must be negotiated",
            ));
        }
        let mut stream = GatewayStream::open(wire.clone(), "$events", json!({})).await?;
        let ready = stream.first().await?;
        if string(&ready, "type")? != "ready" {
            return Err(RemoteError::invalid(
                "Native event stream did not send ready",
            ));
        }
        let client_id = string(&ready, "clientId")?.to_owned();
        let home = ready
            .pointer("/host/home")
            .and_then(Value::as_str)
            .filter(|home| !home.is_empty())
            .map(str::to_owned);
        let (interaction_events, _) = broadcast::channel(256);
        let host = Arc::new(Self {
            wire,
            capabilities,
            home,
            interactions: Arc::new(Mutex::new(Interactions {
                client_id: Some(client_id),
                ..Default::default()
            })),
            interaction_events,
            grants,
            event_task: StdMutex::new(None),
        });
        // The task holds a Weak reference. Dropping the Host cancels it and its
        // connection-owned claims; the stream's Drop closes the official carrier.
        let weak = Arc::downgrade(&host);
        let task = tokio::spawn(async move {
            let mut expiry = tokio::time::interval(Duration::from_millis(250));
            loop {
                let value = tokio::select! {
                    value = stream.next() => value,
                    _ = expiry.tick() => {
                        let Some(host) = weak.upgrade() else { break; };
                        host.expire_questions().await;
                        continue;
                    }
                };
                let Some(host) = weak.upgrade() else {
                    break;
                };
                match value {
                    Ok(Some(value)) => {
                        if host.route_interaction(value).await.is_err() {
                            host.fail_interactions().await;
                            break;
                        }
                    }
                    _ => {
                        host.fail_interactions().await;
                        break;
                    }
                }
            }
        });
        *host
            .event_task
            .lock()
            .map_err(|_| RemoteError::invalid("Native event lifecycle unavailable"))? = Some(task);
        Ok(host)
    }
    pub async fn is_healthy(&self) -> bool {
        !self.wire.is_closed() && !self.interactions.lock().await.failed
    }
    async fn fail_interactions(&self) {
        let mut interactions = self.interactions.lock().await;
        interactions.failed = true;
        interactions.pending.clear();
        interactions.client_id = None;
        let _ = self.interaction_events.send((String::new(), String::new()));
    }
    async fn route_interaction(&self, value: Value) -> Result<()> {
        match string(&value, "type")? {
            "cancel" => {
                self.interactions
                    .lock()
                    .await
                    .pending
                    .remove(string(&value, "eventId")?);
            }
            "waterfall" => {
                let event_id = string(&value, "eventId")?.to_owned();
                {
                    let state = self.interactions.lock().await;
                    if state.consumed.contains(&event_id) || state.pending.contains_key(&event_id) {
                        return Ok(());
                    }
                    if state.consumed.len() >= 16384 {
                        return Err(RemoteError::new(
                            "RATE_LIMITED",
                            "Native interaction ledger is full; reconnect without replay",
                        ));
                    }
                }
                let session_id = string(&value, "agentId")?.to_owned();
                let request = value
                    .get("request")
                    .filter(|v| v.is_object())
                    .ok_or_else(|| RemoteError::invalid("Invalid native interaction request"))?;
                let (questions, description) = match string(&value, "event")? {
                    "approval/request" => (
                        None,
                        request
                            .get("reason")
                            .and_then(Value::as_str)
                            .or_else(|| request.get("toolName").and_then(Value::as_str))
                            .unwrap_or("Harness tool approval")
                            .to_owned(),
                    ),
                    "user-questions/request" => (
                        Some(model::questions(&request["questions"])?),
                        String::new(),
                    ),
                    _ => return Ok(()),
                };
                let mut claim = None;
                let mut deadline = None;
                let mut call_id = None;
                if questions.is_some()
                    && request.pointer("/wait/timed").and_then(Value::as_bool) == Some(true)
                {
                    let native_call = string(&request["wait"], "callId")?.to_owned();
                    let mut stream = GatewayStream::open(
                        self.wire.clone(),
                        "userQuestions/attachWait",
                        json!({"agentId":session_id,"callId":native_call}),
                    )
                    .await?;
                    let remaining = integer(&stream.first().await?, "remainingMs")?;
                    if remaining > 2_147_483_647 {
                        return Err(RemoteError::invalid("Invalid native question deadline"));
                    }
                    deadline = Some(Instant::now() + Duration::from_millis(remaining));
                    call_id = Some(native_call);
                    claim = Some(stream);
                }
                let mut interactions = self.interactions.lock().await;
                if interactions.consumed.contains(&event_id) {
                    return Ok(());
                }
                if interactions.pending.len() >= 128 {
                    return Err(RemoteError::new(
                        "RATE_LIMITED",
                        "Too many pending native interactions",
                    ));
                }
                let client_id = interactions
                    .client_id
                    .clone()
                    .ok_or_else(|| RemoteError::invalid("Native event owner is unavailable"))?;
                interactions.pending.insert(
                    event_id.clone(),
                    PendingInteraction {
                        session_id: session_id.clone(),
                        event_id: Some(event_id.clone()),
                        client_id: Some(client_id),
                        questions,
                        description,
                        call_id,
                        deadline,
                        _claim: claim,
                    },
                );
                let _ = self.interaction_events.send((session_id, event_id));
            }
            // Ready is bound once; owner replacement on a live stream is invalid.
            "ready" => {
                return Err(RemoteError::invalid(
                    "Native event owner changed without reconnect",
                ))
            }
            _ => {}
        }
        Ok(())
    }
    async fn expire_questions(&self) {
        let expired: Vec<_> = {
            let mut state = self.interactions.lock().await;
            let ids: Vec<_> = state
                .pending
                .iter()
                .filter(|(_, item)| {
                    item.deadline
                        .is_some_and(|deadline| deadline <= Instant::now())
                })
                .map(|(id, _)| id.clone())
                .collect();
            ids.into_iter()
                .filter_map(|id| {
                    state.consumed.insert(id.clone());
                    state.pending.remove(&id)
                })
                .collect()
        };
        for pending in expired {
            if let (Some(client), Some(event)) =
                (pending.client_id.as_ref(), pending.event_id.as_ref())
            {
                let _ = call(&self.wire, &id(), "$events/result", json!({"clientId":client,"eventId":event,"outcome":{"kind":"rejected","error":{"name":"UserQuestionError","code":"ASK_TIMED_OUT","message":"The question answer window ended."}}}), CallSemantics::Mutation).await;
            }
        }
    }
    pub async fn workspace_rows(&self) -> Result<Vec<model::WorkspaceRow>> {
        let mut stream =
            GatewayStream::open(self.wire.clone(), "workspace/follow", json!({})).await?;
        let first = stream.first().await;
        let closed = stream.close().await;
        let rows = model::workspaces(&first?)?;
        closed?;
        Ok(rows)
    }
    /// The exact trusted Papers directory for unassigned sessions, derived
    /// only from the Host's own ready-frame home. v3 hosts only.
    pub fn papers_cwd(&self) -> Option<String> {
        if !self.capabilities.contains("harness.remote.v3") {
            return None;
        }
        let home = self.home.as_deref()?;
        let separator = if home.contains('\\') && !home.contains('/') {
            '\\'
        } else {
            '/'
        };
        let trimmed = home.trim_end_matches(['/', '\\']);
        Some(format!("{trimmed}{separator}Agentero{separator}Papers"))
    }
    async fn session_list(&self) -> Result<Vec<model::SessionRow>> {
        model::session_rows(
            call(
                &self.wire,
                &id(),
                "session/list",
                json!({"_request":{}}),
                CallSemantics::ReadOnly,
            )
            .await?,
        )
    }
    pub async fn catalog(&self, workspace: &WorkspaceRef) -> Result<Vec<model::SessionRow>> {
        model::ensure_backend(workspace)?;
        let rows = self.workspace_rows().await?;
        if workspace.workspace_id.0 == model::UNASSIGNED_WORKSPACE_ID {
            // Virtual unassigned scope: root sessions at the exact trusted
            // Papers cwd that no official workspace claims. Never listed for
            // v1 hosts or without a known home.
            let papers = self.papers_cwd().ok_or_else(|| {
                RemoteError::new(
                    "FEATURE_NOT_SUPPORTED",
                    "Unassigned sessions require a v3 Host with a known home",
                )
            })?;
            let owned: HashSet<String> = rows
                .iter()
                .flat_map(|row| row.session_ids.iter().cloned())
                .collect();
            return Ok(model::unassigned_sessions(
                self.session_list().await?,
                &owned,
                &papers,
            ));
        }
        let row = rows
            .into_iter()
            .find(|row| row.workspace_id == workspace.workspace_id.0)
            .ok_or_else(|| {
                RemoteError::new(
                    "WORKSPACE_NOT_FOUND",
                    "Workspace is absent from the official native catalog",
                )
            })?;
        model::sessions(
            call(
                &self.wire,
                &id(),
                "session/list",
                json!({"_request":{}}),
                CallSemantics::ReadOnly,
            )
            .await?,
            &row,
        )
    }
    pub async fn require_session(&self, session: &SessionRef) -> Result<model::SessionRow> {
        self.catalog(&session.workspace)
            .await?
            .into_iter()
            .find(|row| row.session_id == session.session_id.0)
            .ok_or_else(|| {
                RemoteError::new(
                    "SESSION_NOT_IN_WORKSPACE",
                    "Session is absent from this workspace's official native catalog",
                )
            })
    }
    pub async fn session_models(
        &self,
        session: &SessionRef,
    ) -> Result<Option<RemoteSessionModels>> {
        let row = self.require_session(session).await?;
        match self.model_catalog().await? {
            Some(value) => model::session_models(&row, value).map(Some),
            None => Ok(None),
        }
    }

    /// Host-global catalog: no session row exists yet, so the current
    /// selection is the Host default (official sessionModels cache miss).
    pub async fn session_model_catalog(&self) -> Result<Option<RemoteSessionModels>> {
        match self.model_catalog().await? {
            Some(value) => model::host_models(value).map(Some),
            None => Ok(None),
        }
    }

    async fn model_catalog(&self) -> Result<Option<Value>> {
        match call(
            &self.wire,
            &id(),
            "session/modelCatalog",
            json!({}),
            CallSemantics::ReadOnly,
        )
        .await
        {
            Ok(value) => Ok(Some(value)),
            Err(error)
                if matches!(
                    error.code.as_str(),
                    "gateway/method-not-found"
                        | "METHOD_NOT_FOUND"
                        | "METHOD_NOT_ALLOWED"
                        | "FEATURE_NOT_SUPPORTED"
                ) =>
            {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    /// Reuse the Host workspace whose official baseline path matches, or
    /// create it via `workspace/create` (official openRemoteWorkspace). The
    /// caller's journaled operation ID rides on the create call so replay
    /// deduplicates; reuse performs no write at all.
    pub async fn ensure_workspace(
        &self,
        operation: &str,
        host: &HostRef,
        path: &str,
    ) -> Result<WorkspaceRef> {
        if !model::valid_workspace_path(path) {
            return Err(RemoteError::invalid("Invalid native workspace path"));
        }
        if let Some(row) = self
            .workspace_rows()
            .await?
            .into_iter()
            .find(|row| row.path.as_deref() == Some(path))
        {
            return Ok(WorkspaceRef {
                host: host.clone(),
                backend_id: model::BACKEND.into(),
                workspace_id: WorkspaceId(row.workspace_id),
            });
        }
        let value = call(
            &self.wire,
            operation,
            "workspace/create",
            json!({"request":{"path":path}}),
            CallSemantics::Mutation,
        )
        .await?;
        let workspace_id = value
            .pointer("/workspace/workspaceId")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| {
                RemoteError::invalid("Invalid native workspace create result").unknown()
            })?;
        Ok(WorkspaceRef {
            host: host.clone(),
            backend_id: model::BACKEND.into(),
            workspace_id: WorkspaceId(workspace_id.to_owned()),
        })
    }

    pub async fn select_model(
        &self,
        operation: &str,
        session: &SessionRef,
        selection: &RemoteModelSelection,
    ) -> Result<RemoteModelSelection> {
        self.require_session(session).await?;
        let mut request = json!({
            "sessionId": session.session_id.0,
            "provider": selection.provider,
            "model": selection.model,
        });
        if let Some(effort) = &selection.reasoning_effort {
            request["reasoningEffort"] = json!(effort);
        }
        let value = call(
            &self.wire,
            operation,
            "session/selectModel",
            json!({"request": request}),
            CallSemantics::Mutation,
        )
        .await?;
        model::selected_model(&value).map_err(RemoteError::unknown)
    }

    async fn baseline(
        &self,
        session: &SessionRef,
        after: Option<i64>,
    ) -> Result<(GatewayStream, Value, Vec<Value>)> {
        tokio::time::timeout(
            Duration::from_secs(120),
            self.baseline_inner(session, after),
        )
        .await
        .map_err(|_| {
            RemoteError::new(
                "RPC_TIMEOUT",
                "Native history recovery exceeded its total deadline",
            )
        })?
    }
    async fn baseline_inner(
        &self,
        session: &SessionRef,
        after: Option<i64>,
    ) -> Result<(GatewayStream, Value, Vec<Value>)> {
        let mut request = json!({"address":model::address(session),"maxMessages":200});
        if self.capabilities.contains("harness.remote.v3") {
            // Official v3 clients opt into live assistant streaming; the Host
            // then ships snapshot.assistantStream and assistant-stream frames.
            request["assistantStream"] = json!(true);
        }
        let mut stream = GatewayStream::open(
            self.wire.clone(),
            "session/follow",
            json!({"request":request}),
        )
        .await?;
        let snapshot = stream.first().await?;
        if string(&snapshot, "type")? != "snapshot" {
            return Err(RemoteError::invalid(
                "Native session follow did not send snapshot",
            ));
        }
        let through = model::through(&snapshot)?;
        if after.is_some_and(|seq| seq > through) {
            return Err(RemoteError::new(
                "FULL_RESYNC_REQUIRED",
                "Native session cursor is ahead of the authoritative snapshot",
            ));
        }
        let mut records = model::records(&snapshot, through)?;
        let mut has_more = snapshot["hasMore"].as_bool().unwrap();
        let mut total_bytes = serde_json::to_vec(&records)
            .map_err(|_| RemoteError::invalid("Invalid history"))?
            .len();
        while has_more
            && after.is_none_or(|after| {
                records
                    .first()
                    .and_then(|row| row.pointer("/event/seq"))
                    .and_then(Value::as_i64)
                    .is_none_or(|first| first > after + 1)
            })
        {
            let before = records
                .first()
                .and_then(|row| row.pointer("/event/seq"))
                .and_then(Value::as_i64)
                .filter(|seq| *seq > 0)
                .ok_or_else(|| {
                    RemoteError::invalid("Native history pagination made no progress")
                })?;
            let page = call(&self.wire, &id(), "session/page", json!({"request":{"address":model::address(session),"throughSeq":through,"beforeSeq":before,"maxMessages":200}}), CallSemantics::ReadOnly).await?;
            let mut older = model::records(&page, through)?;
            if older.is_empty()
                || older
                    .iter()
                    .any(|row| row["event"]["seq"].as_i64().unwrap() >= before)
            {
                return Err(RemoteError::invalid(
                    "Native history page is empty or overlaps",
                ));
            }
            total_bytes += serde_json::to_vec(&older)
                .map_err(|_| RemoteError::invalid("Invalid history"))?
                .len();
            if total_bytes > 32 * 1024 * 1024 || records.len() + older.len() > 100_000 {
                return Err(RemoteError::new(
                    "RESPONSE_TOO_LARGE",
                    "Complete native history exceeds the adapter's bound",
                ));
            }
            has_more = page["hasMore"].as_bool().unwrap();
            older.append(&mut records);
            records = older;
        }
        self.apply_question_projection(session, snapshot.get("projections"))
            .await?;
        Ok((stream, snapshot, records))
    }
    async fn apply_question_projection(
        &self,
        session: &SessionRef,
        projection: Option<&Value>,
    ) -> Result<()> {
        let Some(value) = projection.and_then(|p| p.pointer("/values/userQuestions")) else {
            return Ok(());
        };
        let active = value
            .get("active")
            .and_then(Value::as_array)
            .ok_or_else(|| RemoteError::invalid("Invalid native question projection"))?;
        let mut state = self.interactions.lock().await;
        let mut continued = HashSet::new();
        for item in active {
            if item.get("state").and_then(Value::as_str) != Some("continued") {
                continue;
            }
            let call_id = string(item, "callId")?.to_owned();
            let request_id = format!("timed-question:{}", json!([session.session_id.0, call_id]));
            continued.insert(request_id.clone());
            state.pending.retain(|_, pending| {
                !(pending.session_id == session.session_id.0
                    && pending.call_id.as_deref() == Some(call_id.as_str())
                    && pending.event_id.is_some())
            });
            if state.consumed.contains(&request_id) {
                continue;
            }
            if state.pending.len() >= 128 {
                return Err(RemoteError::new(
                    "RATE_LIMITED",
                    "Too many pending native interactions",
                ));
            }
            state.pending.insert(
                request_id,
                PendingInteraction {
                    session_id: session.session_id.0.clone(),
                    event_id: None,
                    client_id: None,
                    questions: Some(model::questions(&item["questions"])?),
                    description: String::new(),
                    call_id: Some(call_id),
                    deadline: None,
                    _claim: None,
                },
            );
        }
        state.pending.retain(|request_id, pending| {
            pending.session_id != session.session_id.0
                || pending.event_id.is_some()
                || continued.contains(request_id)
        });
        Ok(())
    }
    async fn pending_events(&self, session: &SessionRef) -> Result<Vec<RemoteEvent>> {
        let state = self.interactions.lock().await;
        if state.failed {
            return Err(RemoteError::new(
                "FULL_RESYNC_REQUIRED",
                "Native interaction owner disconnected",
            ));
        }
        Ok(state
            .pending
            .iter()
            .filter(|(_, item)| item.session_id == session.session_id.0)
            .map(|(id, item)| item.event(session, id))
            .collect())
    }
    pub async fn read(self: &Arc<Self>, session: &SessionRef) -> Result<SessionSnapshot> {
        let row = self.require_session(session).await?;
        let (mut stream, snapshot, records) = self.baseline(session, None).await?;
        stream.close().await?;
        let mut events = records
            .iter()
            .map(|row| model::events(session, &row["event"]))
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        // Durable history first, then the in-flight attempt's streamed suffix.
        events.extend(model::stream_events(session, &snapshot)?);
        events.extend(self.pending_events(session).await?);
        let mut status = model::status(&row, &records, snapshot.get("projections"))?;
        if events
            .iter()
            .any(|event| matches!(event.payload, EventPayload::PermissionRequest { .. }))
        {
            status = TaskStatus::WaitingForPermission;
        } else if events
            .iter()
            .any(|event| matches!(event.payload, EventPayload::QuestionRequest { .. }))
        {
            status = TaskStatus::WaitingForAnswer;
        }
        Ok(SessionSnapshot {
            info: SessionInfo {
                session: session.clone(),
                title: row.title.unwrap_or_default(),
                status,
            },
            events,
            cursor: Some(model::cursor(session, model::through(&snapshot)?)),
            context_usage: model::context_usage(&snapshot),
        })
    }
    pub async fn subscribe(
        self: &Arc<Self>,
        session: &SessionRef,
        after: Option<&EventCursor>,
    ) -> Result<Box<dyn RemoteAgentSubscription>> {
        let row = self.require_session(session).await?;
        let after = after
            .map(|cursor| model::parse_cursor(session, cursor))
            .transpose()?;
        let interactions = self.interaction_events.subscribe();
        let (stream, snapshot, records) = self.baseline(session, after).await?;
        let mut queue = VecDeque::new();
        for record in &records {
            if after.is_none_or(|seq| {
                record["event"]["seq"]
                    .as_i64()
                    .is_some_and(|current| current > seq)
            }) {
                queue.extend(model::events(session, &record["event"])?);
            }
        }
        // The active attempt always sits after the durable cursor; replay its
        // streamed suffix so a mid-run join shows the partial reply at once.
        queue.extend(model::stream_events(session, &snapshot)?);
        queue.extend(self.pending_events(session).await?);
        let status = model::status(&row, &records, snapshot.get("projections"))?;
        let seq = model::through(&snapshot)?;
        queue.push_back(RemoteEvent {
            session: session.clone(),
            source_stream_id: model::source_stream(session),
            source_id: format!("baseline:{seq}:{status:?}"),
            cursor: Some(model::cursor(session, seq)),
            payload: EventPayload::TaskStatus { status },
        });
        let assistant = snapshot
            .pointer("/assistantStream/activeAttempt")
            .and_then(|attempt| {
                Some(ActiveAttempt {
                    attempt_id: attempt.get("attemptId")?.as_str()?.to_owned(),
                    next_index: attempt.get("nextIndex")?.as_u64()?,
                })
            });
        Ok(Box::new(NativeSubscription {
            host: self.clone(),
            session: session.clone(),
            stream,
            interactions,
            queue,
            through: seq,
            assistant,
            cancelled: false,
        }))
    }
    pub async fn respond(
        &self,
        operation: &str,
        session: &SessionRef,
        request_id: &str,
        option: Option<&str>,
        answers: Option<&[QuestionAnswer]>,
    ) -> Result<OperationOutcome> {
        self.require_session(session).await?;
        let pending = {
            let mut state = self.interactions.lock().await;
            if state.failed {
                return Err(RemoteError::new(
                    "PERMISSION_NOT_PENDING",
                    "Native interaction owner disconnected",
                ));
            }
            let pending = state.pending.get(request_id).ok_or_else(|| {
                RemoteError::new(
                    "PERMISSION_NOT_PENDING",
                    "This connection does not own the native interaction",
                )
            })?;
            if pending.session_id != session.session_id.0
                || pending
                    .deadline
                    .is_some_and(|deadline| deadline <= Instant::now())
            {
                return Err(RemoteError::new(
                    "PERMISSION_NOT_PENDING",
                    "Native interaction is expired or belongs to a different session",
                ));
            }
            if pending.questions.is_some() != answers.is_some() {
                return Err(RemoteError::invalid(
                    "Native interaction reply kind mismatch",
                ));
            }
            if let Some(questions) = &pending.questions {
                validate_answers(questions, answers.unwrap())?;
            } else if !matches!(option, Some("allowed-once" | "rejected")) {
                return Err(RemoteError::invalid(
                    "Only one-time approval or rejection is supported",
                ));
            }
            let pending = state.pending.remove(request_id).unwrap();
            state.consumed.insert(request_id.into());
            pending
        };
        let answer = answers.map(|answers| {
            json!({"answers":answers.iter().map(|answer| {
            let mut value = json!({"id":answer.question_id,"selected":answer.selected});
            if let Some(custom) = &answer.custom { value["custom"] = json!(custom); } value
        }).collect::<Vec<_>>() })
        });
        if let (Some(client_id), Some(event_id)) = (&pending.client_id, &pending.event_id) {
            call(&self.wire, operation, "$events/result", json!({"clientId":client_id,"eventId":event_id,"outcome":{"kind":"result","value":answer.unwrap_or_else(|| json!(option.unwrap()))}}), CallSemantics::Mutation).await?;
            Ok(OperationOutcome::Accepted)
        } else {
            let result = call(
                &self.wire,
                operation,
                "userQuestions/answer",
                json!({"agentId":session.session_id.0,"callId":pending.call_id,"answer":answer}),
                CallSemantics::Mutation,
            )
            .await?;
            match result.as_bool() {
                Some(true) => Ok(OperationOutcome::Accepted),
                Some(false) => Err(RemoteError::new(
                    "PERMISSION_NOT_PENDING",
                    "Native question was already answered or expired",
                )),
                None => Err(
                    RemoteError::invalid("Invalid native question answer acknowledgement")
                        .unknown(),
                ),
            }
        }
    }
    pub async fn upload(
        &self,
        operation: &str,
        upload: &AttachmentUpload,
    ) -> Result<RemoteAttachment> {
        if !self.capabilities.contains("workspace.upload.v1") {
            return Err(RemoteError::new(
                "FEATURE_NOT_SUPPORTED",
                "Host did not negotiate workspace upload",
            ));
        }
        self.require_session(&upload.session).await?;
        if !valid_file_name(&upload.file_name) || upload.size_bytes > UPLOAD_LIMIT {
            return Err(RemoteError::invalid(
                "Invalid workspace upload name or size",
            ));
        }
        let mut file = self.grants.take(&upload.local_path)?;
        let expected = upload.size_bytes;
        let bytes = tokio::task::spawn_blocking(move || -> Result<Vec<u8>> {
            let before = file
                .metadata()
                .map_err(|_| RemoteError::invalid("Cannot inspect upload grant"))?;
            if !before.is_file() || before.len() != expected || before.len() > UPLOAD_LIMIT {
                return Err(RemoteError::invalid("Granted file size changed"));
            }
            file.seek(SeekFrom::Start(0))
                .map_err(|_| RemoteError::invalid("Cannot seek granted upload file"))?;
            let mut bytes = Vec::with_capacity(expected as usize);
            (&mut file)
                .take(expected + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| RemoteError::invalid("Cannot read granted upload file"))?;
            let after = file
                .metadata()
                .map_err(|_| RemoteError::invalid("Cannot recheck granted upload file"))?;
            let changed =
                before.len() != after.len() || before.modified().ok() != after.modified().ok();
            #[cfg(unix)]
            let changed = {
                use std::os::unix::fs::MetadataExt;
                changed
                    || before.ctime() != after.ctime()
                    || before.ctime_nsec() != after.ctime_nsec()
            };
            if bytes.len() as u64 != expected || changed {
                return Err(RemoteError::invalid("Granted upload file changed"));
            }
            Ok(bytes)
        })
        .await
        .map_err(|_| RemoteError::invalid("Upload reader failed"))??;
        let upload_id = id();
        let scope = json!({"sessionId":upload.session.session_id.0,"uploadId":upload_id});
        let deadline = Instant::now() + Duration::from_secs(300);
        let send = |method: &'static str, params: Value, suffix: String| {
            let wire = self.wire.clone();
            let request_id = format!("{operation}:{suffix}");
            async move {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(RemoteError::new(
                        "RPC_TIMEOUT",
                        "Workspace upload deadline exceeded",
                    )
                    .unknown());
                }
                wire.request(
                    &request_id,
                    method,
                    params,
                    CallSemantics::Mutation,
                    remaining.min(RPC_BUDGET),
                )
                .await
            }
        };
        let mut opened = scope.clone();
        opened["name"] = json!(upload.file_name);
        opened["totalBytes"] = json!(bytes.len());
        let result = async {
            let opened = send("workspace.upload.open", opened, "open".into()).await?;
            if string(&opened, "uploadId")? != upload_id {
                return Err(RemoteError::invalid("Workspace upload identity mismatch").unknown());
            }
            let chunk_bytes = integer(&opened, "chunkBytes")? as usize;
            if !(1..=65536).contains(&chunk_bytes) {
                return Err(RemoteError::invalid("Invalid workspace upload chunk size").unknown());
            }
            use base64::Engine;
            let mut offset = 0;
            for (index, chunk) in bytes.chunks(chunk_bytes).enumerate() {
                let mut params = scope.clone();
                params["offset"] = json!(offset);
                params["data"] = json!(base64::engine::general_purpose::STANDARD.encode(chunk));
                let result =
                    send("workspace.upload.chunk", params, format!("chunk-{index}")).await?;
                offset += chunk.len();
                if string(&result, "uploadId")? != upload_id
                    || integer(&result, "receivedBytes")? != offset as u64
                {
                    return Err(RemoteError::invalid(
                        "Workspace upload chunk acknowledgement mismatch",
                    )
                    .unknown());
                }
            }
            let result = send("workspace.upload.commit", scope.clone(), "commit".into()).await?;
            if integer(&result, "bytes").map_err(RemoteError::unknown)? != expected
                || !valid_file_name(string(&result, "name").map_err(RemoteError::unknown)?)
            {
                return Err(
                    RemoteError::invalid("Invalid workspace upload commit result").unknown(),
                );
            }
            let path = string(&result, "path").map_err(RemoteError::unknown)?;
            if path
                != format!(
                    "./{}",
                    string(&result, "name").map_err(RemoteError::unknown)?
                )
            {
                return Err(RemoteError::invalid("Invalid committed workspace path").unknown());
            }
            Ok(RemoteAttachment {
                session: upload.session.clone(),
                path: path.into(),
                size_bytes: expected,
            })
        }
        .await;
        if result.is_err() {
            let _ = self
                .wire
                .request(
                    &id(),
                    "workspace.upload.abort",
                    scope,
                    CallSemantics::Mutation,
                    Duration::from_secs(5),
                )
                .await;
        }
        result
    }
}
impl Drop for NativeHost {
    fn drop(&mut self) {
        if let Ok(task) = self.event_task.get_mut() {
            if let Some(task) = task.take() {
                task.abort();
            }
        }
    }
}
fn validate_answers(questions: &[Question], answers: &[QuestionAnswer]) -> Result<()> {
    if questions.len() != answers.len() {
        return Err(RemoteError::invalid(
            "Answer every native question, including skipped questions",
        ));
    }
    let mut ids = HashSet::new();
    for answer in answers {
        let question = questions
            .iter()
            .find(|question| question.id == answer.question_id)
            .ok_or_else(|| RemoteError::invalid("Answer refers to an unknown native question"))?;
        if !ids.insert(&answer.question_id)
            || (!question.multiple && answer.selected.len() > 1)
            || (!question.allows_custom
                && answer
                    .custom
                    .as_ref()
                    .is_some_and(|custom| !custom.is_empty()))
            || answer
                .selected
                .iter()
                .any(|label| !question.options.contains(label))
            || answer.selected.iter().collect::<HashSet<_>>().len() != answer.selected.len()
        {
            return Err(RemoteError::invalid("Invalid native question selections"));
        }
    }
    Ok(())
}
fn valid_file_name(name: &str) -> bool {
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    !name.is_empty()
        && name.len() <= 220
        && name != "."
        && name != ".."
        && !name.starts_with(".dsh-upload-")
        && !name.ends_with(['.', ' '])
        && !name
            .chars()
            .any(|c| c.is_control() || "/\\<>:\"|?*".contains(c))
        && !matches!(stem.as_str(), "con" | "prn" | "aux" | "nul")
        && !(stem.len() == 4
            && (stem.starts_with("com") || stem.starts_with("lpt"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
}
/// The in-flight assistant attempt tracked from `assistant-stream` frames.
/// Live chunks are transient: they never advance the durable cursor.
struct ActiveAttempt {
    attempt_id: String,
    next_index: u64,
}
struct NativeSubscription {
    host: Arc<NativeHost>,
    session: SessionRef,
    stream: GatewayStream,
    interactions: broadcast::Receiver<(String, String)>,
    queue: VecDeque<RemoteEvent>,
    through: i64,
    assistant: Option<ActiveAttempt>,
    cancelled: bool,
}
#[async_trait]
impl RemoteAgentSubscription for NativeSubscription {
    async fn next(
        &mut self,
    ) -> std::result::Result<Option<RemoteEvent>, crate::core::error::AppError> {
        async {
            loop {
                if let Some(event) = self.queue.pop_front() { return Ok(Some(event)); }
                if self.cancelled { return Ok(None); }
                tokio::select! {
                    interaction = self.interactions.recv() => {
                        let (session, id) = interaction.map_err(|_| RemoteError::new("FULL_RESYNC_REQUIRED", "Native interaction updates were lost"))?;
                        if session.is_empty() { return Err(RemoteError::new("FULL_RESYNC_REQUIRED", "Native interaction owner disconnected")); }
                        if session == self.session.session_id.0 { if let Some(pending) = self.host.interactions.lock().await.pending.get(&id) { self.queue.push_back(pending.event(&self.session, &id)); } }
                    }
                    value = self.stream.next() => {
                        let Some(value) = value? else { return Err(RemoteError::new("FULL_RESYNC_REQUIRED", "Native session stream ended; reload its snapshot")); };
                        match value.get("type").and_then(Value::as_str) {
                            Some("event" | "chunks") => {
                                let event = &value["event"]; let seq = integer(event, "seq")? as i64;
                                if seq <= self.through { continue; } self.through = seq;
                                self.queue.extend(model::events(&self.session, event)?);
                            }
                            Some("assistant-stream") => {
                                // Official pumpSessionFollow: start/chunk/end frames
                                // carry transient deltas; durable gating is untouched.
                                let Some(frame) = value.get("frame").filter(|f| f.is_object()) else { continue; };
                                match frame.get("type").and_then(Value::as_str) {
                                    Some("start") => {
                                        if let Some(attempt_id) = frame.get("attemptId").and_then(Value::as_str) {
                                            self.assistant = Some(ActiveAttempt {
                                                attempt_id: attempt_id.to_owned(),
                                                next_index: frame.get("nextIndex").and_then(Value::as_u64).unwrap_or(0),
                                            });
                                        }
                                    }
                                    Some("chunk") => {
                                        let matches_attempt = self.assistant.as_ref().is_some_and(|attempt| {
                                            frame.get("attemptId").and_then(Value::as_str) == Some(attempt.attempt_id.as_str())
                                        });
                                        if !matches_attempt { continue; }
                                        let next_index = self.assistant.as_ref().map_or(0, |attempt| attempt.next_index);
                                        let index = frame.get("index").and_then(Value::as_u64).unwrap_or(next_index);
                                        if index < next_index { continue; }
                                        let event = frame.get("chunk").and_then(|chunk| {
                                            model::live_chunk_event(&self.session, &self.assistant.as_ref()?.attempt_id, index, chunk)
                                        });
                                        if let Some(attempt) = self.assistant.as_mut() { attempt.next_index = index + 1; }
                                        if let Some(event) = event { self.queue.push_back(event); }
                                    }
                                    Some("end") if self.assistant.as_ref().is_some_and(|attempt| {
                                        frame.get("attemptId").and_then(Value::as_str) == Some(attempt.attempt_id.as_str())
                                    }) => {
                                        self.assistant = None;
                                    }
                                    _ => {}
                                }
                            }
                            Some("projection") => {
                                if value.get("key").and_then(Value::as_str) == Some("userQuestions") {
                                    let projection = json!({"values":{"userQuestions":value["value"]}});
                                    self.host.apply_question_projection(&self.session, Some(&projection)).await?;
                                    self.queue.extend(self.host.pending_events(&self.session).await?);
                                }
                            }
                            _ => return Err(RemoteError::invalid("Unsupported native session follow frame")),
                        }
                    }
                }
            }
        }.await.map_err(|error: RemoteError| crate::core::error::AppError::domain("dsh_remote", error.to_string()))
    }
    async fn cancel(&mut self) -> std::result::Result<(), crate::core::error::AppError> {
        self.cancelled = true;
        self.queue.clear();
        self.stream
            .close()
            .await
            .map_err(|error| crate::core::error::AppError::domain("dsh_remote", error.to_string()))
    }
}
