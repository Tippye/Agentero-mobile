use super::*;
use crate::core::error::AppError;
use async_trait::async_trait;

pub(crate) fn unsupported(capability: Capability) -> AppError {
    AppError::domain(
        "remote_agent_unsupported",
        format!("Remote Agent capability {capability:?} is not supported"),
    )
}

/// A cancellable subscription owned by Rust, independent of any WebView.
/// Dropping it must release local resources; cancel also closes the remote stream.
#[async_trait]
pub trait RemoteAgentSubscription: Send {
    async fn next(&mut self) -> Result<Option<RemoteEvent>, AppError>;
    async fn cancel(&mut self) -> Result<(), AppError>;
}

/// Implemented by built-in integration adapters, not a dynamic plugin ABI.
///
/// Adapters own authentication, trust checks, transport, host-level connection
/// sharing and protocol mapping. They advertise only actually negotiated
/// capabilities and return fully namespaced references. A connection config
/// version must match the descriptor. Descriptor identity is stable after
/// registration; negotiated capabilities may change on resume. Network calls
/// must have bounded timeouts so lifecycle calls cannot wait indefinitely behind
/// a stalled request. Disconnect must terminate the connection's subscriptions.
#[async_trait]
pub trait RemoteAgentProvider: Send + Sync {
    fn descriptor(&self) -> ProviderDescriptor;
    async fn connect(&self, request: &ConnectRequest) -> Result<ConnectionInfo, AppError>;
    async fn disconnect(&self, connection: &ConnectionRef) -> Result<(), AppError>;

    /// Advisory path change for existing transports only. Never connect an
    /// account, refresh credentials, or replay a pending operation.
    async fn network_changed(&self) {}

    /// Re-establish the transport only. Never replay pending mutations.
    async fn resume(&self, _connection: &ConnectionRef) -> Result<ConnectionInfo, AppError> {
        Err(unsupported(Capability::Resume))
    }

    async fn list_hosts(&self, _connection: &ConnectionRef) -> Result<Vec<HostInfo>, AppError> {
        Err(unsupported(Capability::ListHosts))
    }

    async fn list_workspaces(&self, _host: &HostRef) -> Result<Vec<WorkspaceInfo>, AppError> {
        Err(unsupported(Capability::ListWorkspaces))
    }

    async fn list_sessions(&self, _workspace: &WorkspaceRef) -> Result<Vec<SessionInfo>, AppError> {
        Err(unsupported(Capability::ListSessions))
    }

    async fn read_session(&self, _session: &SessionRef) -> Result<SessionSnapshot, AppError> {
        Err(unsupported(Capability::ReadSession))
    }

    async fn session_models(
        &self,
        _session: &SessionRef,
    ) -> Result<Option<RemoteSessionModels>, AppError> {
        Ok(None)
    }

    /// Host-global model catalog for the model picker before a first send.
    /// `None` means the Host does not expose a catalog on this route.
    async fn host_models(&self, _host: &HostRef) -> Result<Option<RemoteSessionModels>, AppError> {
        Ok(None)
    }

    /// Snapshot cursor is opaque. The adapter must use official history/stream
    /// recovery and close any race between baseline and subscription. There is
    /// no assumption that the backend supports an event replay buffer.
    async fn subscribe(
        &self,
        _session: &SessionRef,
        _after: Option<&EventCursor>,
    ) -> Result<Box<dyn RemoteAgentSubscription>, AppError> {
        Err(unsupported(Capability::Subscribe))
    }

    /// Called at most once per operation ID by one service instance. Return
    /// Rejected only for a known refusal; post-dispatch uncertainty is Unknown.
    /// An error is conservatively treated as Unknown by the service.
    async fn execute(&self, request: &OperationRequest) -> Result<OperationOutcome, AppError> {
        Err(unsupported(request.mutation.capability()))
    }

    /// Read-only lookup. NotFound (including after restart) never permits retry.
    async fn lookup_receipt(
        &self,
        _connection: &ConnectionRef,
        _operation_id: &OperationId,
    ) -> Result<ReceiptLookup, AppError> {
        Err(unsupported(Capability::OperationReceipts))
    }
}
