use serde::{Deserialize, Serialize};

pub const DEFAULT_PROVIDER_ID: &str = "ds-harness-remote";
pub const CONTRACT_VERSION: u32 = 1;

macro_rules! identifier {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, specta::Type)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }
    };
}

identifier!(ProviderId);
identifier!(ConnectionId);
identifier!(HostId);
identifier!(WorkspaceId);
identifier!(SessionId);
identifier!(BackendId);
identifier!(OperationId);

/// A configured connection instance, shared by all application windows.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionRef {
    pub provider_id: ProviderId,
    pub connection_id: ConnectionId,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct HostRef {
    pub connection: ConnectionRef,
    pub host_id: HostId,
}

/// Backend is part of identity: Harness, ACP and Codex IDs never alias.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceRef {
    pub host: HostRef,
    pub backend_id: BackendId,
    pub workspace_id: WorkspaceId,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SessionRef {
    pub workspace: WorkspaceRef,
    pub session_id: SessionId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum Capability {
    ListHosts,
    ListWorkspaces,
    ListSessions,
    CreateSession,
    ReadSession,
    SendMessage,
    Cancel,
    Subscribe,
    RespondPermission,
    RespondQuestion,
    UploadAttachment,
    OperationReceipts,
    Resume,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum AuthenticationMethod {
    DevicePairing,
    Browser,
    Password,
    ExistingCredential,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentLimits {
    /// Effective limit for the currently negotiated route, in bytes.
    pub max_bytes: u64,
    /// Empty means the adapter has no MIME allowlist, not that upload is enabled.
    pub mime_types: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    pub operations: Vec<Capability>,
    pub authentication: Vec<AuthenticationMethod>,
    pub backends: Vec<BackendId>,
    pub attachments: Option<AttachmentLimits>,
}

impl Capabilities {
    pub fn supports(&self, operation: Capability) -> bool {
        self.operations.contains(&operation)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ProviderDescriptor {
    pub provider_id: ProviderId,
    pub display_name: String,
    pub contract_version: u32,
    pub config_version: u32,
}

/// Configuration and credentials stay in the adapter's Rust-owned store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ConnectRequest {
    pub connection: ConnectionRef,
    pub config_version: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionInfo {
    pub connection: ConnectionRef,
    pub capabilities: Capabilities,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct HostInfo {
    pub host: HostRef,
    pub name: String,
    /// `None` when the Server did not report presence and the descriptor lookup
    /// failed; absence is not an offline verdict.
    pub online: Option<bool>,
    /// This device has already approved and pinned the Host's current identity.
    #[serde(default)]
    pub pinned: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceInfo {
    pub workspace: WorkspaceRef,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum TaskStatus {
    Idle,
    Running,
    WaitingForPermission,
    WaitingForAnswer,
    Completed,
    Failed,
    Cancelled,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub session: SessionRef,
    pub title: String,
    /// Idle and Completed are distinct; an idle transport is not task success.
    pub status: TaskStatus,
}

/// Adapter-owned opaque token. It is never parsed or incremented by the service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct EventCursor {
    pub stream_id: String,
    pub value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum ConnectionStatus {
    Connected,
    Reconnecting,
    Disconnected,
    AuthenticationRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct Question {
    pub id: String,
    pub prompt: String,
    pub options: Vec<String>,
    pub multiple: bool,
    pub allows_custom: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct QuestionAnswer {
    pub question_id: String,
    pub selected: Vec<String>,
    pub custom: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum EventPayload {
    Message {
        role: String,
        text: String,
        /// Model reasoning attached to this reply; rendered as a disclosure.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning: Option<String>,
    },
    /// One native tool invocation. `call` and `result` share the same source ID
    /// so clients fold them into a single activity row.
    Tool {
        call_id: String,
        name: String,
        /// `running`, `finished` or `failed`.
        state: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        arguments: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<String>,
    },
    TaskStatus {
        status: TaskStatus,
    },
    PermissionRequest {
        request_id: String,
        description: String,
        options: Vec<String>,
    },
    QuestionRequest {
        request_id: String,
        questions: Vec<Question>,
    },
    ConnectionStatus {
        status: ConnectionStatus,
    },
}

/// Source IDs must identify immutable events and survive history/stream replay.
/// Stream ID separates source epochs where a backend may reuse event IDs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct RemoteEvent {
    pub session: SessionRef,
    pub source_stream_id: String,
    pub source_id: String,
    pub cursor: Option<EventCursor>,
    pub payload: EventPayload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SessionSnapshot {
    pub info: SessionInfo,
    pub events: Vec<RemoteEvent>,
    /// A subscription opened with this token must cover changes after this
    /// baseline. Adapters implement this using their official history/streams.
    pub cursor: Option<EventCursor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentUpload {
    pub session: SessionRef,
    /// The integration validates file authority, regular-file status and size.
    pub local_path: String,
    pub file_name: String,
    pub mime_type: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct RemoteAttachment {
    pub session: SessionRef,
    pub path: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Mutation {
    CreateSession {
        workspace: WorkspaceRef,
        title: Option<String>,
    },
    SendMessage {
        session: SessionRef,
        text: String,
        attachments: Vec<RemoteAttachment>,
    },
    Cancel {
        session: SessionRef,
    },
    RespondPermission {
        session: SessionRef,
        request_id: String,
        option_id: String,
    },
    RespondQuestion {
        session: SessionRef,
        request_id: String,
        answers: Vec<QuestionAnswer>,
    },
    UploadAttachment {
        upload: AttachmentUpload,
    },
}

impl Mutation {
    pub fn workspace(&self) -> &WorkspaceRef {
        match self {
            Self::CreateSession { workspace, .. } => workspace,
            Self::SendMessage { session, .. }
            | Self::Cancel { session }
            | Self::RespondPermission { session, .. }
            | Self::RespondQuestion { session, .. } => &session.workspace,
            Self::UploadAttachment { upload } => &upload.session.workspace,
        }
    }

    pub fn capability(&self) -> Capability {
        match self {
            Self::CreateSession { .. } => Capability::CreateSession,
            Self::SendMessage { .. } => Capability::SendMessage,
            Self::Cancel { .. } => Capability::Cancel,
            Self::RespondPermission { .. } => Capability::RespondPermission,
            Self::RespondQuestion { .. } => Capability::RespondQuestion,
            Self::UploadAttachment { .. } => Capability::UploadAttachment,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct OperationRequest {
    pub operation_id: OperationId,
    pub mutation: Mutation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum OperationResult {
    SessionCreated { session: SessionRef },
    AttachmentUploaded { attachment: RemoteAttachment },
    Acknowledged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum OperationOutcome {
    /// Queued/accepted does not establish completion of the remote task.
    Accepted,
    Succeeded {
        result: OperationResult,
    },
    Rejected {
        code: String,
        message: String,
    },
    /// May already have executed. Inspect the receipt/history; never resend.
    Unknown {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "status", content = "outcome", rename_all = "camelCase")]
pub enum ReceiptLookup {
    Found(OperationOutcome),
    /// Missing/expired receipts do not establish that an operation did not run.
    NotFound,
}
