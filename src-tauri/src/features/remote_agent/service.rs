use super::provider::unsupported;
use super::*;
use crate::core::error::AppError;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};
use tokio::sync::Mutex;

fn invalid(message: impl Into<String>) -> AppError {
    AppError::domain("remote_agent_invalid", message)
}

pub(super) fn boundary() -> AppError {
    AppError::domain(
        "remote_agent_namespace",
        "Remote Agent reference crossed its namespace",
    )
}

pub(super) fn identifier(value: &str) -> Result<(), AppError> {
    if value.trim().is_empty() || value.len() > 1024 || value.chars().any(char::is_control) {
        return Err(invalid("Invalid opaque Remote Agent identifier"));
    }
    Ok(())
}

pub(super) fn validate_model_selection(selection: &RemoteModelSelection) -> Result<(), AppError> {
    identifier(&selection.provider)?;
    identifier(&selection.model)?;
    if let Some(effort) = &selection.reasoning_effort {
        identifier(effort)?;
    }
    Ok(())
}

pub(super) fn connection_ref(connection: &ConnectionRef) -> Result<(), AppError> {
    identifier(&connection.provider_id.0)?;
    identifier(&connection.connection_id.0)
}

fn host_ref(host: &HostRef) -> Result<(), AppError> {
    connection_ref(&host.connection)?;
    identifier(&host.host_id.0)
}

pub(super) fn workspace_ref(workspace: &WorkspaceRef) -> Result<(), AppError> {
    host_ref(&workspace.host)?;
    identifier(&workspace.backend_id.0)?;
    identifier(&workspace.workspace_id.0)
}

pub(super) fn session_ref(session: &SessionRef) -> Result<(), AppError> {
    workspace_ref(&session.workspace)?;
    identifier(&session.session_id.0)
}

pub(super) fn operation_id(id: &OperationId) -> Result<(), AppError> {
    if !(16..=128).contains(&id.0.len())
        || !id
            .0
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(invalid(
            "Operation ID must contain 16–128 ASCII letters, digits, '_' or '-'",
        ));
    }
    Ok(())
}

fn require(info: &ConnectionInfo, capability: Capability) -> Result<(), AppError> {
    if !info.capabilities.supports(capability) {
        return Err(unsupported(capability));
    }
    Ok(())
}

fn backend(info: &ConnectionInfo, workspace: &WorkspaceRef) -> Result<(), AppError> {
    if workspace.host.connection != info.connection {
        return Err(boundary());
    }
    if !info.capabilities.backends.contains(&workspace.backend_id) {
        return Err(AppError::domain(
            "remote_agent_backend_unsupported",
            "Remote Agent backend is unavailable",
        ));
    }
    Ok(())
}

fn connection_info(info: &ConnectionInfo, expected: &ConnectionRef) -> Result<(), AppError> {
    if &info.connection != expected {
        return Err(boundary());
    }
    for backend in &info.capabilities.backends {
        identifier(&backend.0)?;
    }
    if info.capabilities.supports(Capability::UploadAttachment)
        && info.capabilities.attachments.is_none()
    {
        return Err(invalid(
            "Upload capability requires effective attachment limits",
        ));
    }
    Ok(())
}

fn event(event: &RemoteEvent, expected: &SessionRef) -> Result<(), AppError> {
    if &event.session != expected {
        return Err(boundary());
    }
    identifier(&event.source_stream_id)?;
    identifier(&event.source_id)?;
    if let Some(cursor) = &event.cursor {
        identifier(&cursor.stream_id)?;
        identifier(&cursor.value)?;
    }
    Ok(())
}

fn snapshot(snapshot: &mut SessionSnapshot, expected: &SessionRef) -> Result<(), AppError> {
    if &snapshot.info.session != expected {
        return Err(boundary());
    }
    if let Some(cursor) = &snapshot.cursor {
        identifier(&cursor.stream_id)?;
        identifier(&cursor.value)?;
    }
    let mut seen = HashSet::new();
    for item in &snapshot.events {
        event(item, expected)?;
    }
    snapshot
        .events
        .retain(|item| seen.insert((item.source_stream_id.clone(), item.source_id.clone())));
    Ok(())
}

#[derive(Clone)]
struct RegisteredProvider {
    descriptor: ProviderDescriptor,
    adapter: Arc<dyn RemoteAgentProvider>,
}

struct Registry {
    default_provider: ProviderId,
    providers: HashMap<ProviderId, RegisteredProvider>,
}

#[derive(Default)]
struct ConnectionState {
    info: Option<ConnectionInfo>,
    active: bool,
    operations: HashMap<OperationId, JournalEntry>,
}

impl ConnectionState {
    fn connected(&self) -> Result<&ConnectionInfo, AppError> {
        self.info.as_ref().filter(|_| self.active).ok_or_else(|| {
            AppError::domain(
                "remote_agent_disconnected",
                "Remote Agent connection is not active",
            )
        })
    }
}

/// One process-owned service for all windows and background tasks. Connection
/// calls are serialized per configured instance, never under the registry lock.
/// The injected journal owns operation identity across service lifetimes. A local
/// cache retains conservative Unknown states even if a storage future is dropped.
/// Neither disconnects nor missing official receipts authorize redispatch.
pub struct RemoteAgentService {
    registry: RwLock<Registry>,
    connections: Mutex<HashMap<ConnectionRef, Arc<Mutex<ConnectionState>>>>,
    journal: Arc<dyn OperationJournal>,
}

impl Default for RemoteAgentService {
    fn default() -> Self {
        Self::with_journal(Arc::new(MemoryOperationJournal::default()))
    }
}

impl RemoteAgentService {
    pub fn with_journal(journal: Arc<dyn OperationJournal>) -> Self {
        Self {
            registry: RwLock::new(Registry {
                default_provider: ProviderId::from(DEFAULT_PROVIDER_ID),
                providers: HashMap::new(),
            }),
            connections: Mutex::new(HashMap::new()),
            journal,
        }
    }

    pub fn register(&self, adapter: Arc<dyn RemoteAgentProvider>) -> Result<(), AppError> {
        let descriptor = adapter.descriptor();
        identifier(&descriptor.provider_id.0)?;
        if descriptor.contract_version != CONTRACT_VERSION || descriptor.config_version == 0 {
            return Err(invalid(
                "Unsupported Remote Agent contract or configuration version",
            ));
        }
        let mut registry = self
            .registry
            .write()
            .map_err(|_| invalid("Provider registry is unavailable"))?;
        if registry.providers.contains_key(&descriptor.provider_id) {
            return Err(AppError::domain(
                "remote_agent_duplicate_provider",
                "Provider ID is already registered",
            ));
        }
        registry.providers.insert(
            descriptor.provider_id.clone(),
            RegisteredProvider {
                descriptor,
                adapter,
            },
        );
        Ok(())
    }

    pub fn providers(&self) -> Result<Vec<ProviderDescriptor>, AppError> {
        let registry = self
            .registry
            .read()
            .map_err(|_| invalid("Provider registry is unavailable"))?;
        let mut providers: Vec<_> = registry
            .providers
            .values()
            .map(|p| p.descriptor.clone())
            .collect();
        providers.sort_by(|a, b| a.provider_id.0.cmp(&b.provider_id.0));
        Ok(providers)
    }

    pub fn default_provider(&self) -> Result<ProviderId, AppError> {
        Ok(self
            .registry
            .read()
            .map_err(|_| invalid("Provider registry is unavailable"))?
            .default_provider
            .clone())
    }

    pub fn set_default_provider(&self, id: &ProviderId) -> Result<(), AppError> {
        let mut registry = self
            .registry
            .write()
            .map_err(|_| invalid("Provider registry is unavailable"))?;
        if !registry.providers.contains_key(id) {
            return Err(AppError::domain(
                "remote_agent_provider_not_found",
                "Remote Agent provider is not registered",
            ));
        }
        registry.default_provider = id.clone();
        Ok(())
    }

    fn provider(&self, connection: &ConnectionRef) -> Result<RegisteredProvider, AppError> {
        connection_ref(connection)?;
        self.registry
            .read()
            .map_err(|_| invalid("Provider registry is unavailable"))?
            .providers
            .get(&connection.provider_id)
            .cloned()
            .ok_or_else(|| {
                AppError::domain(
                    "remote_agent_provider_not_found",
                    "Remote Agent provider is not registered",
                )
            })
    }

    async fn state(&self, connection: &ConnectionRef) -> Arc<Mutex<ConnectionState>> {
        self.connections
            .lock()
            .await
            .entry(connection.clone())
            .or_default()
            .clone()
    }

    /// Notify registered transport owners without changing connection or journal
    /// state. Provider callbacks cannot authorize an explicit reconnect.
    pub async fn network_changed(&self) {
        let adapters: Vec<_> = match self.registry.read() {
            Ok(registry) => registry
                .providers
                .values()
                .map(|provider| provider.adapter.clone())
                .collect(),
            Err(_) => return,
        };
        for adapter in adapters {
            adapter.network_changed().await;
        }
    }

    pub async fn connect(&self, request: &ConnectRequest) -> Result<ConnectionInfo, AppError> {
        let provider = self.provider(&request.connection)?;
        if provider.descriptor.config_version != request.config_version {
            return Err(invalid("Remote Agent configuration version mismatch"));
        }
        let state = self.state(&request.connection).await;
        let mut state = state.lock().await;
        if state.active {
            return Ok(state.connected()?.clone());
        }
        let info = provider.adapter.connect(request).await?;
        connection_info(&info, &request.connection)?;
        state.info = Some(info.clone());
        state.active = true;
        Ok(info)
    }

    pub async fn disconnect(&self, connection: &ConnectionRef) -> Result<(), AppError> {
        let provider = self.provider(connection)?;
        let state = self.state(connection).await;
        let mut state = state.lock().await;
        if !state.active {
            return Ok(());
        }
        // Fail closed even if cleanup errors or its future is cancelled.
        state.active = false;
        provider.adapter.disconnect(connection).await
    }

    pub async fn resume(&self, connection: &ConnectionRef) -> Result<ConnectionInfo, AppError> {
        let provider = self.provider(connection)?;
        let state = self.state(connection).await;
        let mut state = state.lock().await;
        let previous = state
            .info
            .as_ref()
            .ok_or_else(|| invalid("Connect before resuming"))?;
        require(previous, Capability::Resume)?;
        state.active = false;
        let info = provider.adapter.resume(connection).await?;
        connection_info(&info, connection)?;
        state.info = Some(info.clone());
        state.active = true;
        Ok(info)
    }

    pub async fn list_hosts(&self, connection: &ConnectionRef) -> Result<Vec<HostInfo>, AppError> {
        let provider = self.provider(connection)?;
        let state = self.state(connection).await;
        let state = state.lock().await;
        require(state.connected()?, Capability::ListHosts)?;
        let hosts = provider.adapter.list_hosts(connection).await?;
        for item in &hosts {
            host_ref(&item.host)?;
            if &item.host.connection != connection {
                return Err(boundary());
            }
        }
        Ok(hosts)
    }

    pub async fn list_workspaces(&self, host: &HostRef) -> Result<Vec<WorkspaceInfo>, AppError> {
        host_ref(host)?;
        let provider = self.provider(&host.connection)?;
        let state = self.state(&host.connection).await;
        let state = state.lock().await;
        let info = state.connected()?;
        require(info, Capability::ListWorkspaces)?;
        let workspaces = provider.adapter.list_workspaces(host).await?;
        for item in &workspaces {
            workspace_ref(&item.workspace)?;
            if &item.workspace.host != host {
                return Err(boundary());
            }
            backend(info, &item.workspace)?;
        }
        Ok(workspaces)
    }

    pub async fn list_sessions(
        &self,
        workspace: &WorkspaceRef,
    ) -> Result<Vec<SessionInfo>, AppError> {
        workspace_ref(workspace)?;
        let provider = self.provider(&workspace.host.connection)?;
        let state = self.state(&workspace.host.connection).await;
        let state = state.lock().await;
        let info = state.connected()?;
        require(info, Capability::ListSessions)?;
        backend(info, workspace)?;
        let sessions = provider.adapter.list_sessions(workspace).await?;
        for item in &sessions {
            session_ref(&item.session)?;
            if &item.session.workspace != workspace {
                return Err(boundary());
            }
        }
        Ok(sessions)
    }

    pub async fn read_session(&self, session: &SessionRef) -> Result<SessionSnapshot, AppError> {
        session_ref(session)?;
        let provider = self.provider(&session.workspace.host.connection)?;
        let state = self.state(&session.workspace.host.connection).await;
        let state = state.lock().await;
        let info = state.connected()?;
        require(info, Capability::ReadSession)?;
        backend(info, &session.workspace)?;
        let mut result = provider.adapter.read_session(session).await?;
        snapshot(&mut result, session)?;
        Ok(result)
    }

    pub async fn session_models(
        &self,
        session: &SessionRef,
    ) -> Result<Option<RemoteSessionModels>, AppError> {
        session_ref(session)?;
        let provider = self.provider(&session.workspace.host.connection)?;
        let state = self.state(&session.workspace.host.connection).await;
        let state = state.lock().await;
        let info = state.connected()?;
        backend(info, &session.workspace)?;
        if !info.capabilities.supports(Capability::SelectModel) {
            return Ok(None);
        }
        let result = provider.adapter.session_models(session).await?;
        if let Some(models) = &result {
            validate_model_selection(&models.current)?;
        }
        Ok(result)
    }

    /// Explicit reconnect recovery: return a complete official baseline and a
    /// subscriber seeded with its source IDs. Each window gets its own baseline
    /// and de-duplication set. No mutations are dispatched during recovery.
    pub async fn recover_session(
        &self,
        session: &SessionRef,
    ) -> Result<RecoveredSession, AppError> {
        session_ref(session)?;
        let provider = self.provider(&session.workspace.host.connection)?;
        let state = self.state(&session.workspace.host.connection).await;
        let state = state.lock().await;
        let info = state.connected()?;
        require(info, Capability::ReadSession)?;
        require(info, Capability::Subscribe)?;
        backend(info, &session.workspace)?;
        let mut baseline = provider.adapter.read_session(session).await?;
        snapshot(&mut baseline, session)?;
        let inner = provider
            .adapter
            .subscribe(session, baseline.cursor.as_ref())
            .await?;
        let seen = baseline
            .events
            .iter()
            .map(|e| (e.source_stream_id.clone(), e.source_id.clone()))
            .collect();
        Ok(RecoveredSession {
            subscription: SessionSubscription {
                inner,
                session: session.clone(),
                seen,
                closed: false,
            },
            snapshot: baseline,
        })
    }

    async fn recorded(
        &self,
        state: &mut ConnectionState,
        connection: &ConnectionRef,
        id: &OperationId,
    ) -> Result<Option<JournalEntry>, AppError> {
        if let Some(entry) = state.operations.get(id) {
            return Ok(Some(entry.clone()));
        }
        let entry = self.journal.get(connection, id).await?;
        if let Some(entry) = &entry {
            entry.validate()?;
            if &entry.connection != connection || &entry.operation_id != id {
                return Err(boundary());
            }
            state.operations.insert(id.clone(), entry.clone());
        }
        Ok(entry)
    }

    pub async fn execute(&self, request: &OperationRequest) -> Result<OperationOutcome, AppError> {
        operation_id(&request.operation_id)?;
        let workspace = request.mutation.workspace();
        workspace_ref(workspace)?;
        let connection = &workspace.host.connection;
        let provider = self.provider(connection)?;
        let state = self.state(connection).await;
        let mut state = state.lock().await;
        // Check durable identity before connectivity/capabilities. Restart,
        // cancellation and capability changes never make a used ID dispatchable.
        if let Some(previous) = self
            .recorded(&mut state, connection, &request.operation_id)
            .await?
        {
            return previous.check_request(request);
        }
        let info = state.connected()?;
        require(info, request.mutation.capability())?;
        backend(info, workspace)?;
        validate_mutation(&request.mutation, info)?;
        let intent = JournalEntry::intent(request)?;
        // Cache first to quarantine the ID if the commit future is cancelled.
        // A provider call is allowed ONLY after a successful durable reservation.
        state
            .operations
            .insert(request.operation_id.clone(), intent.clone());
        let reservation = self.journal.reserve(&intent).await?;
        reservation.entry.validate()?;
        if reservation.entry.connection != *connection
            || reservation.entry.operation_id != request.operation_id
        {
            return Err(boundary());
        }
        if !reservation.inserted {
            state
                .operations
                .insert(request.operation_id.clone(), reservation.entry.clone());
            return reservation.entry.check_request(request);
        }
        if reservation.entry != intent {
            return Err(journal_unavailable());
        }
        let outcome = match provider.adapter.execute(request).await {
            Ok(outcome)
                if validate_outcome(&outcome, Some(&request.mutation), connection).is_ok() =>
            {
                journal_outcome(&outcome)
            }
            _ => unknown_outcome(),
        };
        let proposed = match intent.with_outcome(&outcome) {
            Ok(proposed) => proposed,
            Err(_) => return Ok(unknown_outcome()),
        };
        // Do not publish a definitive local outcome unless its journal update
        // completed. An error/cancellation leaves the cached durable Unknown.
        match self.journal.update(&proposed).await {
            Ok(stored) => {
                if stored.validate().is_err() {
                    return Ok(unknown_outcome());
                }
                if stored.connection != intent.connection
                    || stored.operation_id != intent.operation_id
                    || stored.request_digest != intent.request_digest
                    || stored.context != intent.context
                {
                    return Ok(unknown_outcome());
                }
                let outcome = stored.outcome.clone();
                state
                    .operations
                    .insert(request.operation_id.clone(), stored);
                Ok(outcome)
            }
            Err(_) => Ok(unknown_outcome()),
        }
    }

    pub async fn local_outcome(
        &self,
        connection: &ConnectionRef,
        id: &OperationId,
    ) -> Result<Option<OperationOutcome>, AppError> {
        connection_ref(connection)?;
        operation_id(id)?;
        let state = self.state(connection).await;
        let mut state = state.lock().await;
        Ok(self
            .recorded(&mut state, connection, id)
            .await?
            .map(|record| record.outcome))
    }

    /// Local-only restoration: no provider registration, credential refresh or RPC.
    pub async fn local_operations(
        &self,
        connection: &ConnectionRef,
    ) -> Result<Vec<JournalEntry>, AppError> {
        connection_ref(connection)?;
        let state = self.state(connection).await;
        let state = state.lock().await;
        let mut entries = HashMap::new();
        for entry in self.journal.list(connection).await? {
            entry.validate()?;
            if &entry.connection != connection {
                return Err(boundary());
            }
            if entries.insert(entry.operation_id.clone(), entry).is_some() {
                return Err(journal_unavailable());
            }
        }
        // A failed/cancelled outcome commit may be more uncertain than a later
        // readback. Preserve this process's conservative result until lookup.
        entries.extend(state.operations.clone());
        let mut entries: Vec<_> = entries.into_values().collect();
        entries.sort_by(|a, b| a.operation_id.0.cmp(&b.operation_id.0));
        Ok(entries)
    }

    /// Receipt lookup only; NotFound never clears the journal. Also works for a
    /// retained operation ID after application restart, without redispatching it.
    pub async fn lookup_receipt(
        &self,
        connection: &ConnectionRef,
        id: &OperationId,
    ) -> Result<ReceiptLookup, AppError> {
        let provider = self.provider(connection)?;
        operation_id(id)?;
        let state = self.state(connection).await;
        let mut state = state.lock().await;
        let known = self
            .recorded(&mut state, connection, id)
            .await?
            .unwrap_or_else(|| JournalEntry::imported(connection, id));
        state.operations.insert(id.clone(), known.clone());
        // Import is durable even if the following connection/capability check
        // fails. Receipt queries can never accidentally authorize a resend.
        let reserved = self
            .journal
            .reserve(&known.with_outcome(&unknown_outcome())?)
            .await?;
        reserved.entry.validate()?;
        if &reserved.entry.connection != connection || &reserved.entry.operation_id != id {
            return Err(boundary());
        }
        let known = if reserved.inserted {
            known
        } else {
            reserved.entry
        };
        state.operations.insert(id.clone(), known.clone());
        require(state.connected()?, Capability::OperationReceipts)?;
        let receipt = provider.adapter.lookup_receipt(connection, id).await?;
        if let ReceiptLookup::Found(outcome) = &receipt {
            super::journal::validate_context_outcome(outcome, known.context.as_ref(), connection)?;
            let proposed = known.with_outcome(outcome)?;
            match self.journal.update(&proposed).await {
                Ok(stored) => {
                    stored.validate()?;
                    if stored.connection != known.connection
                        || stored.operation_id != known.operation_id
                        || stored.request_digest != known.request_digest
                    {
                        return Err(boundary());
                    }
                    let outcome = stored.outcome.clone();
                    state.operations.insert(id.clone(), stored);
                    return Ok(ReceiptLookup::Found(outcome));
                }
                Err(_) => return Ok(ReceiptLookup::Found(known.outcome)),
            }
        }
        Ok(receipt)
    }
}

fn validate_mutation(mutation: &Mutation, info: &ConnectionInfo) -> Result<(), AppError> {
    match mutation {
        Mutation::CreateSession { .. } => (),
        Mutation::SendMessage {
            session,
            attachments,
            ..
        } => {
            session_ref(session)?;
            for attachment in attachments {
                if &attachment.session != session {
                    return Err(boundary());
                }
            }
        }
        Mutation::SelectModel { session, selection } => {
            session_ref(session)?;
            validate_model_selection(selection)?;
        }
        Mutation::Cancel { session }
        | Mutation::RespondPermission { session, .. }
        | Mutation::RespondQuestion { session, .. } => session_ref(session)?,
        Mutation::UploadAttachment { upload } => {
            session_ref(&upload.session)?;
            let limits = info
                .capabilities
                .attachments
                .as_ref()
                .ok_or_else(|| unsupported(Capability::UploadAttachment))?;
            if upload.size_bytes > limits.max_bytes {
                return Err(AppError::domain(
                    "remote_agent_attachment_too_large",
                    "Attachment exceeds the negotiated route limit",
                ));
            }
            if !limits.mime_types.is_empty() && !limits.mime_types.contains(&upload.mime_type) {
                return Err(AppError::domain(
                    "remote_agent_attachment_type",
                    "Attachment MIME type is not supported",
                ));
            }
            if upload.local_path.is_empty() || upload.file_name.is_empty() {
                return Err(invalid("Attachment path and name are required"));
            }
        }
    }
    Ok(())
}

fn validate_outcome(
    outcome: &OperationOutcome,
    mutation: Option<&Mutation>,
    connection: &ConnectionRef,
) -> Result<(), AppError> {
    if let OperationOutcome::Succeeded { result } = outcome {
        match result {
            OperationResult::SessionCreated { session } => {
                session_ref(session)?;
                if &session.workspace.host.connection != connection {
                    return Err(boundary());
                }
                if let Some(mutation) = mutation {
                    match mutation {
                        Mutation::CreateSession { workspace, .. }
                            if workspace == &session.workspace => {}
                        _ => return Err(boundary()),
                    }
                }
            }
            OperationResult::AttachmentUploaded { attachment } => {
                session_ref(&attachment.session)?;
                if &attachment.session.workspace.host.connection != connection {
                    return Err(boundary());
                }
                if let Some(mutation) = mutation {
                    match mutation {
                        Mutation::UploadAttachment { upload }
                            if upload.session == attachment.session => {}
                        _ => return Err(boundary()),
                    }
                }
            }
            OperationResult::ModelSelected { session, selected } => {
                session_ref(session)?;
                validate_model_selection(selected)?;
                if &session.workspace.host.connection != connection {
                    return Err(boundary());
                }
                if let Some(mutation) = mutation {
                    if !matches!(mutation, Mutation::SelectModel { session: target, .. } if target == session)
                    {
                        return Err(boundary());
                    }
                }
            }
            OperationResult::Acknowledged => {
                if matches!(
                    mutation,
                    Some(
                        Mutation::CreateSession { .. }
                            | Mutation::UploadAttachment { .. }
                            | Mutation::SelectModel { .. }
                    )
                ) {
                    return Err(invalid("Completed operation is missing its result"));
                }
            }
        }
    }
    Ok(())
}

pub struct RecoveredSession {
    pub snapshot: SessionSnapshot,
    pub subscription: SessionSubscription,
}

/// De-duplication is per subscriber and source epoch. Keys, not event bodies,
/// are retained until cancellation/drop so history overlaps cannot lose messages.
/// Stream failure/EOF closes this subscriber; callers explicitly recover again.
pub struct SessionSubscription {
    inner: Box<dyn RemoteAgentSubscription>,
    session: SessionRef,
    seen: HashSet<(String, String)>,
    closed: bool,
}

impl SessionSubscription {
    pub async fn next(&mut self) -> Result<Option<RemoteEvent>, AppError> {
        if self.closed {
            return Ok(None);
        }
        loop {
            let next = self.inner.next().await;
            match next {
                Ok(Some(item)) => {
                    if let Err(error) = event(&item, &self.session) {
                        self.closed = true;
                        let _ = self.inner.cancel().await;
                        return Err(error);
                    }
                    if self
                        .seen
                        .insert((item.source_stream_id.clone(), item.source_id.clone()))
                    {
                        return Ok(Some(item));
                    }
                }
                Ok(None) => {
                    self.closed = true;
                    return Ok(None);
                }
                Err(error) => {
                    self.closed = true;
                    return Err(error);
                }
            }
        }
    }

    pub async fn cancel(&mut self) -> Result<(), AppError> {
        self.closed = true;
        self.inner.cancel().await
    }
}
