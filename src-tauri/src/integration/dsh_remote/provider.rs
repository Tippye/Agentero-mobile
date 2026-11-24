//! Native Harness provider over official Typert Remote carriers at upstream 11064af.
#[cfg(test)]
#[path = "network_tests.rs"]
mod network_tests;
use super::{
    api::ServerApi,
    crypto::DeviceIdentity,
    gateway::{call, string},
    native::{NativeHost, UploadGrants, UPLOAD_LIMIT},
    native_model as model,
    relay::RelayChannel,
    rpc::{CallSemantics, RpcClient},
    RemoteError, Result,
};
use crate::{core::error::AppError, features::remote_agent::*};
use async_trait::async_trait;
use serde_json::json;
use std::{collections::HashMap, fs::File, sync::Arc, time::Duration};
use tokio::sync::Mutex;

pub struct DshConnectionConfig {
    pub api: ServerApi,
    pub identity: DeviceIdentity,
    pub access_token: String,
    /// Only keys explicitly approved by the user and stored outside this module.
    pub pinned_hosts: HashMap<String, String>,
}
struct HostRuntime {
    rpc: RpcClient,
    native: Arc<NativeHost>,
}
struct Receipt {
    fingerprint: String,
    outcome: OperationOutcome,
}
struct Connection {
    config: DshConnectionConfig,
    connected: bool,
    hosts: HashMap<String, HostRuntime>,
    grants: Arc<UploadGrants>,
    receipts: HashMap<String, Receipt>,
}
#[derive(Default)]
pub struct DshRemoteProvider {
    connections: Mutex<HashMap<String, Arc<Mutex<Connection>>>>,
}
impl DshRemoteProvider {
    /// Called by the Rust credential/config owner, never directly through IPC.
    pub async fn configure(
        &self,
        connection_id: String,
        config: DshConnectionConfig,
    ) -> Result<()> {
        if connection_id.is_empty() {
            return Err(RemoteError::invalid("Empty configured connection ID"));
        }
        let mut connections = self.connections.lock().await;
        if let Some(connection) = connections.get(&connection_id) {
            let mut connection = connection.lock().await;
            if connection.connected {
                return Err(RemoteError::new(
                    "REQUEST_CONFLICT",
                    "Disconnect before replacing Remote credentials or trust pins",
                ));
            }
            connection.config = config;
            return Ok(());
        }
        connections.insert(
            connection_id,
            Arc::new(Mutex::new(Connection {
                config,
                connected: false,
                hosts: HashMap::new(),
                grants: Arc::default(),
                receipts: HashMap::new(),
            })),
        );
        Ok(())
    }
    /// Replace only the bearer used by subsequent REST/authentication requests.
    /// The caller validates and atomically stores the complete refreshed pair;
    /// an existing Noise channel, its claims, upload grants and receipts survive.
    pub async fn update_access_token(
        &self,
        reference: &ConnectionRef,
        access_token: String,
    ) -> Result<()> {
        if !(16..=8192).contains(&access_token.len())
            || access_token.bytes().any(|byte| byte.is_ascii_control())
        {
            return Err(RemoteError::invalid(
                "Invalid refreshed Remote access token",
            ));
        }
        let state = self.connection(reference).await?;
        state.lock().await.config.access_token = access_token;
        Ok(())
    }
    /// Logout drops credentials, identity, approved pins, pending upload grants
    /// and launch-memory receipts after closing every shared Host channel.
    pub async fn forget(&self, reference: &ConnectionRef) -> Result<()> {
        if reference.provider_id.0 != DEFAULT_PROVIDER_ID {
            return Err(RemoteError::invalid("Provider namespace mismatch"));
        }
        let state = self
            .connections
            .lock()
            .await
            .remove(&reference.connection_id.0);
        if let Some(state) = state {
            let mut state = state.lock().await;
            state.connected = false;
            let hosts = std::mem::take(&mut state.hosts);
            for host in hosts.into_values() {
                host.rpc.close().await;
            }
            state.receipts.clear();
            state.grants = Arc::default();
            state.config.access_token.clear();
            state.config.pinned_hosts.clear();
        }
        Ok(())
    }
    async fn connection(&self, reference: &ConnectionRef) -> Result<Arc<Mutex<Connection>>> {
        if reference.provider_id.0 != DEFAULT_PROVIDER_ID {
            return Err(RemoteError::invalid("Provider namespace mismatch"));
        }
        self.connections
            .lock()
            .await
            .get(&reference.connection_id.0)
            .cloned()
            .ok_or_else(|| RemoteError::new("AUTH_REQUIRED", "Remote connection is not configured"))
    }
    /// Trusted native file-picker integration grants an already-open regular file.
    /// UploadAttachment.local_path alone never authorizes arbitrary filesystem reads.
    pub async fn grant_upload_file(
        &self,
        connection: &ConnectionRef,
        local_path: String,
        file: File,
    ) -> Result<()> {
        self.connection(connection)
            .await?
            .lock()
            .await
            .grants
            .grant(local_path, file)
    }
    async fn native_host(&self, host: &HostRef) -> Result<Arc<NativeHost>> {
        self.connect_host(host).await?;
        let state = self.connection(&host.connection).await?;
        let state = state.lock().await;
        state
            .hosts
            .get(&host.host_id.0)
            .filter(|host| !host.rpc.is_closed())
            .map(|host| host.native.clone())
            .ok_or_else(|| RemoteError::new("TRANSPORT_CLOSED", "Native Host disconnected"))
    }
    /// Share a single authenticated Host channel among all application windows.
    pub async fn connect_host(&self, host: &HostRef) -> Result<RpcClient> {
        let state = self.connection(&host.connection).await?;
        let mut state = state.lock().await;
        if !state.connected {
            return Err(RemoteError::new(
                "AUTH_REQUIRED",
                "Connect the account before selecting a Host",
            ));
        }
        if let Some(runtime) = state.hosts.get(&host.host_id.0) {
            if runtime.native.is_healthy().await {
                return Ok(runtime.rpc.clone());
            }
        }
        if state
            .hosts
            .iter()
            .any(|(id, active)| id != &host.host_id.0 && !active.rpc.is_closed())
        {
            return Err(RemoteError::new("CONNECTION_IN_USE", "This client identity already controls another Host; select_host must explicitly switch it"));
        }
        let stale = std::mem::take(&mut state.hosts);
        for stale in stale.into_values() {
            stale.rpc.close().await;
        }
        let config = &state.config;
        let pinned = config.pinned_hosts.get(&host.host_id.0).ok_or_else(|| {
            RemoteError::new(
                "TRUST_REQUIRED",
                "Approve and persist this Host identity before connecting",
            )
        })?;
        let channel = RelayChannel::connect(
            &config.api,
            &config.identity,
            &config.access_token,
            &host.host_id.0,
            pinned,
            Duration::from_secs(10),
        )
        .await?;
        let rpc = RpcClient::start(channel);
        let setup = async {
            let capabilities = rpc.describe(Duration::from_secs(30)).await?;
            NativeHost::start(Arc::new(rpc.clone()), capabilities, state.grants.clone()).await
        }
        .await;
        let native = match setup {
            Ok(native) => native,
            Err(error) => {
                rpc.close().await;
                return Err(error);
            }
        };
        state.hosts.insert(
            host.host_id.0.clone(),
            HostRuntime {
                rpc: rpc.clone(),
                native,
            },
        );
        Ok(rpc)
    }
    /// Explicitly switch the single active Host for this registered client.
    /// The caller must refresh shared-service capabilities through resume after
    /// selection; no pending operation or interaction answer is replayed.
    pub async fn select_host(&self, host: &HostRef) -> Result<ConnectionInfo> {
        let state = self.connection(&host.connection).await?;
        {
            let mut state = state.lock().await;
            if !state.connected {
                return Err(RemoteError::new(
                    "AUTH_REQUIRED",
                    "Connect the account before selecting a Host",
                ));
            }
            let previous = std::mem::take(&mut state.hosts);
            for runtime in previous.into_values() {
                runtime.rpc.close().await;
            }
        }
        self.connect_host(host).await?;
        let state = state.lock().await;
        Ok(info(&host.connection, &state))
    }
    async fn mutate(&self, request: &OperationRequest) -> Result<OperationOutcome> {
        let workspace = request.mutation.workspace();
        model::ensure_backend(workspace)?;
        let host = self.native_host(&workspace.host).await?;
        Self::mutate_on_host(&host, request).await
    }
    pub(crate) async fn mutate_on_host(
        host: &Arc<NativeHost>,
        request: &OperationRequest,
    ) -> Result<OperationOutcome> {
        let operation = &request.operation_id.0;
        match &request.mutation {
            Mutation::CreateSession { workspace, title } => {
                if title.is_some() {
                    return Err(RemoteError::new("FEATURE_NOT_SUPPORTED", "Native session/create has no title argument; creation with a requested title is unsupported"));
                }
                if !host
                    .workspace_rows()
                    .await?
                    .iter()
                    .any(|row| row.workspace_id == workspace.workspace_id.0)
                {
                    return Err(RemoteError::new(
                        "WORKSPACE_NOT_FOUND",
                        "Workspace is absent from the official native catalog",
                    ));
                }
                let value = call(
                    &host.wire,
                    operation,
                    "session/create",
                    json!({"request":{"workspaceId":workspace.workspace_id.0}}),
                    CallSemantics::Mutation,
                )
                .await?;
                let id = string(&value, "sessionId").map_err(RemoteError::unknown)?;
                Ok(OperationOutcome::Succeeded {
                    result: OperationResult::SessionCreated {
                        session: SessionRef {
                            workspace: workspace.clone(),
                            session_id: id.into(),
                        },
                    },
                })
            }
            Mutation::SendMessage {
                session,
                text,
                attachments,
            } => {
                host.require_session(session).await?;
                let mut prompt = text.clone();
                for attachment in attachments {
                    if attachment.session != *session
                        || !attachment.path.starts_with("./")
                        || attachment.path[2..].contains(['/', '\\', '\n', '\r', '\0'])
                        || attachment.size_bytes > UPLOAD_LIMIT
                    {
                        return Err(RemoteError::invalid("Attachment reference belongs to another session or is not a workspace upload path"));
                    }
                }
                if !attachments.is_empty() {
                    prompt.push_str("\n\nAttached files in this session's workspace:\n");
                    for attachment in attachments {
                        prompt.push_str("- ");
                        prompt.push_str(&attachment.path);
                        prompt.push('\n');
                    }
                }
                if prompt.trim().is_empty() {
                    return Err(RemoteError::invalid(
                        "Native prompt must contain text or workspace attachment references",
                    ));
                }
                call(&host.wire, operation, "session/prompt", json!({"request":{"sessionId":session.session_id.0,"requestId":operation,"mode":"queue","content":[{"type":"text","text":prompt}]}}), CallSemantics::Mutation).await?;
                Ok(OperationOutcome::Accepted)
            }
            Mutation::Cancel { session } => {
                host.require_session(session).await?;
                let value = call(
                    &host.wire,
                    operation,
                    "session/cancel",
                    json!({"request":{"sessionId":session.session_id.0}}),
                    CallSemantics::Mutation,
                )
                .await?;
                if value.get("accepted").and_then(serde_json::Value::as_bool) != Some(true) {
                    return Err(RemoteError::invalid(
                        "Native cancel did not acknowledge acceptance",
                    )
                    .unknown());
                }
                Ok(OperationOutcome::Accepted)
            }
            Mutation::SelectModel { session, selection } => Ok(OperationOutcome::Succeeded {
                result: OperationResult::ModelSelected {
                    session: session.clone(),
                    selected: Box::new(host.select_model(operation, session, selection).await?),
                },
            }),
            Mutation::RespondPermission {
                session,
                request_id,
                option_id,
            } => {
                host.respond(operation, session, request_id, Some(option_id), None)
                    .await
            }
            Mutation::RespondQuestion {
                session,
                request_id,
                answers,
            } => {
                host.respond(operation, session, request_id, None, Some(answers))
                    .await
            }
            Mutation::UploadAttachment { upload } => Ok(OperationOutcome::Succeeded {
                result: OperationResult::AttachmentUploaded {
                    attachment: host.upload(operation, upload).await?,
                },
            }),
        }
    }
}
fn app_error(error: RemoteError) -> AppError {
    AppError::domain("dsh_remote", error.to_string())
}
fn info(reference: &ConnectionRef, state: &Connection) -> ConnectionInfo {
    let mut operations = vec![
        Capability::ListHosts,
        Capability::Resume,
        Capability::OperationReceipts,
    ];
    let native = state.hosts.values().any(|host| !host.rpc.is_closed());
    if native {
        operations.extend([
            Capability::ListWorkspaces,
            Capability::ListSessions,
            Capability::CreateSession,
            Capability::ReadSession,
            Capability::SelectModel,
            Capability::SendMessage,
            Capability::Cancel,
            Capability::Subscribe,
            Capability::RespondPermission,
            Capability::RespondQuestion,
        ]);
    }
    let upload = state.hosts.values().any(|host| {
        !host.rpc.is_closed() && host.native.capabilities.contains("workspace.upload.v1")
    });
    if upload {
        operations.push(Capability::UploadAttachment);
    }
    ConnectionInfo {
        connection: reference.clone(),
        capabilities: Capabilities {
            operations,
            authentication: vec![
                AuthenticationMethod::Browser,
                AuthenticationMethod::ExistingCredential,
            ],
            backends: if native {
                vec![model::BACKEND.into()]
            } else {
                vec![]
            },
            attachments: upload.then_some(AttachmentLimits {
                max_bytes: UPLOAD_LIMIT,
                mime_types: vec![],
            }),
        },
    }
}
#[async_trait]
impl RemoteAgentProvider for DshRemoteProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            provider_id: DEFAULT_PROVIDER_ID.into(),
            display_name: "DSH Remote".into(),
            contract_version: CONTRACT_VERSION,
            config_version: 1,
        }
    }
    async fn connect(
        &self,
        request: &ConnectRequest,
    ) -> std::result::Result<ConnectionInfo, AppError> {
        if request.config_version != 1 {
            return Err(app_error(RemoteError::invalid(
                "Unsupported DSH configuration version",
            )));
        }
        let state = self
            .connection(&request.connection)
            .await
            .map_err(app_error)?;
        let hosts = {
            let mut state = state.lock().await;
            let hosts = state
                .config
                .api
                .list_hosts(&state.config.access_token)
                .await
                .map_err(app_error)?;
            state.connected = true;
            if state.hosts.values().any(|runtime| !runtime.rpc.is_closed()) {
                vec![]
            } else {
                let mut candidates = hosts
                    .into_iter()
                    .filter(|host| {
                        // Absent presence is not an offline verdict; the pinned
                        // descriptor check in connect_host still fails closed.
                        host.online != Some(false)
                            && state.config.pinned_hosts.contains_key(&host.device_id)
                    })
                    .collect::<Vec<_>>();
                candidates.sort_by(|left, right| left.device_id.cmp(&right.device_id));
                candidates.into_iter().take(1).collect::<Vec<_>>()
            }
        };
        for host in hosts {
            if let Err(error) = self
                .connect_host(&HostRef {
                    connection: request.connection.clone(),
                    host_id: HostId(host.device_id),
                })
                .await
            {
                let _ = self.disconnect(&request.connection).await;
                return Err(app_error(error));
            }
        }
        let state = state.lock().await;
        Ok(info(&request.connection, &state))
    }
    async fn disconnect(&self, reference: &ConnectionRef) -> std::result::Result<(), AppError> {
        let state = self.connection(reference).await.map_err(app_error)?;
        let mut state = state.lock().await;
        for host in state.hosts.values() {
            host.rpc.close().await;
        }
        state.hosts.clear();
        state.connected = false;
        Ok(())
    }
    async fn network_changed(&self) {
        let connections: Vec<_> = self.connections.lock().await.values().cloned().collect();
        for connection in connections {
            let state = connection.lock().await;
            if state.connected {
                // Keep the selected Host and all receipts/grants. The next scoped
                // read lazily reconnects that exact Host with existing credentials.
                for host in state.hosts.values() {
                    host.rpc.close().await;
                }
            }
        }
    }
    async fn resume(
        &self,
        reference: &ConnectionRef,
    ) -> std::result::Result<ConnectionInfo, AppError> {
        self.connect(&ConnectRequest {
            connection: reference.clone(),
            config_version: 1,
        })
        .await
    }
    async fn list_hosts(
        &self,
        reference: &ConnectionRef,
    ) -> std::result::Result<Vec<HostInfo>, AppError> {
        let state = self.connection(reference).await.map_err(app_error)?;
        let (api, token, pins) = {
            let state = state.lock().await;
            if !state.connected {
                return Err(app_error(RemoteError::new(
                    "AUTH_REQUIRED",
                    "Remote account is disconnected",
                )));
            }
            (
                state.config.api.clone(),
                state.config.access_token.clone(),
                state.config.pinned_hosts.clone(),
            )
        };
        let hosts = api.list_hosts(&token).await.map_err(app_error)?;
        // The device list omits presence on newer Servers. Resolve each Host's
        // descriptor + presence like the official clients; a lookup failure is
        // surfaced as unknown, never silently downgraded to offline.
        let mut rows = Vec::with_capacity(hosts.len());
        for host in hosts {
            let (online, pinned) = match api.host(&token, &host.device_id).await {
                Ok(peer) => (
                    peer.device.online.or(host.online),
                    pins.get(&host.device_id)
                        .is_some_and(|pin| pin == &peer.identity_key),
                ),
                Err(_) => (host.online, pins.contains_key(&host.device_id)),
            };
            rows.push(HostInfo {
                host: HostRef {
                    connection: reference.clone(),
                    host_id: HostId(host.device_id),
                },
                name: host.name,
                online,
                pinned,
            });
        }
        Ok(rows)
    }
    async fn list_workspaces(
        &self,
        reference: &HostRef,
    ) -> std::result::Result<Vec<WorkspaceInfo>, AppError> {
        let host = self.native_host(reference).await.map_err(app_error)?;
        Ok(host
            .workspace_rows()
            .await
            .map_err(app_error)?
            .into_iter()
            .map(|row| WorkspaceInfo {
                workspace: WorkspaceRef {
                    host: reference.clone(),
                    backend_id: model::BACKEND.into(),
                    workspace_id: WorkspaceId(row.workspace_id),
                },
                name: row.title,
            })
            .collect())
    }
    async fn list_sessions(
        &self,
        reference: &WorkspaceRef,
    ) -> std::result::Result<Vec<SessionInfo>, AppError> {
        model::ensure_backend(reference).map_err(app_error)?;
        let host = self.native_host(&reference.host).await.map_err(app_error)?;
        Ok(host
            .catalog(reference)
            .await
            .map_err(app_error)?
            .into_iter()
            .map(|row| SessionInfo {
                session: SessionRef {
                    workspace: reference.clone(),
                    session_id: SessionId(row.session_id),
                },
                title: row.title.unwrap_or_default(),
                status: if row.running {
                    TaskStatus::Running
                } else {
                    TaskStatus::Idle
                },
            })
            .collect())
    }
    async fn read_session(
        &self,
        reference: &SessionRef,
    ) -> std::result::Result<SessionSnapshot, AppError> {
        model::ensure_backend(&reference.workspace).map_err(app_error)?;
        self.native_host(&reference.workspace.host)
            .await
            .map_err(app_error)?
            .read(reference)
            .await
            .map_err(app_error)
    }
    async fn session_models(
        &self,
        reference: &SessionRef,
    ) -> std::result::Result<Option<RemoteSessionModels>, AppError> {
        model::ensure_backend(&reference.workspace).map_err(app_error)?;
        self.native_host(&reference.workspace.host)
            .await
            .map_err(app_error)?
            .session_models(reference)
            .await
            .map_err(app_error)
    }
    async fn subscribe(
        &self,
        reference: &SessionRef,
        after: Option<&EventCursor>,
    ) -> std::result::Result<Box<dyn RemoteAgentSubscription>, AppError> {
        model::ensure_backend(&reference.workspace).map_err(app_error)?;
        self.native_host(&reference.workspace.host)
            .await
            .map_err(app_error)?
            .subscribe(reference, after)
            .await
            .map_err(app_error)
    }
    async fn execute(
        &self,
        request: &OperationRequest,
    ) -> std::result::Result<OperationOutcome, AppError> {
        let reference = &request.mutation.workspace().host.connection;
        let state = self.connection(reference).await.map_err(app_error)?;
        let fingerprint = serde_json::to_string(&request.mutation)
            .map_err(|_| app_error(RemoteError::invalid("Cannot fingerprint native mutation")))?;
        {
            let mut state = state.lock().await;
            if let Some(receipt) = state.receipts.get(&request.operation_id.0) {
                if receipt.fingerprint != fingerprint {
                    return Ok(OperationOutcome::Rejected {
                        code: "REQUEST_CONFLICT".into(),
                        message: "Operation ID already belongs to a different mutation".into(),
                    });
                }
                return Ok(receipt.outcome.clone());
            }
            if state.receipts.len() >= 512 {
                return Ok(OperationOutcome::Rejected {
                    code: "RATE_LIMITED".into(),
                    message: "Native operation receipt ledger is full".into(),
                });
            }
            state.receipts.insert(
                request.operation_id.0.clone(),
                Receipt {
                    fingerprint,
                    outcome: OperationOutcome::Unknown {
                        reason: "Native operation is pending or its caller stopped observing"
                            .into(),
                    },
                },
            );
        }
        let outcome = match self.mutate(request).await {
            Ok(outcome) => outcome,
            Err(error) if error.outcome_unknown => OperationOutcome::Unknown {
                reason: error.to_string(),
            },
            Err(error) => OperationOutcome::Rejected {
                code: error.code,
                message: error.message,
            },
        };
        if let Some(receipt) = state.lock().await.receipts.get_mut(&request.operation_id.0) {
            receipt.outcome = outcome.clone();
        }
        Ok(outcome)
    }
    async fn lookup_receipt(
        &self,
        reference: &ConnectionRef,
        operation: &OperationId,
    ) -> std::result::Result<ReceiptLookup, AppError> {
        let state = self.connection(reference).await.map_err(app_error)?;
        let state = state.lock().await;
        Ok(state
            .receipts
            .get(&operation.0)
            .map(|receipt| ReceiptLookup::Found(receipt.outcome.clone()))
            .unwrap_or(ReceiptLookup::NotFound))
    }
}
