//! Durable operation identity and outcomes, deliberately excluding request bodies.
use super::*;
use crate::core::error::AppError;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Mutex;

pub const JOURNAL_VERSION: u32 = 1;
const UNCERTAIN: &str = "Operation may already have executed; inspect its receipt";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum OperationContext {
    CreateSession {
        workspace: WorkspaceRef,
    },
    EnsureWorkspace {
        host: HostRef,
        path: String,
    },
    SendMessage {
        session: SessionRef,
    },
    Cancel {
        session: SessionRef,
    },
    SelectModel {
        session: SessionRef,
    },
    RespondPermission {
        session: SessionRef,
        request_id: String,
    },
    RespondQuestion {
        session: SessionRef,
        request_id: String,
    },
    UploadAttachment {
        session: SessionRef,
    },
}
impl OperationContext {
    pub fn workspace(&self) -> Option<&WorkspaceRef> {
        match self {
            Self::CreateSession { workspace } => Some(workspace),
            Self::EnsureWorkspace { .. } => None,
            Self::SendMessage { session }
            | Self::Cancel { session }
            | Self::SelectModel { session }
            | Self::RespondPermission { session, .. }
            | Self::RespondQuestion { session, .. }
            | Self::UploadAttachment { session } => Some(&session.workspace),
        }
    }
    pub fn host(&self) -> &HostRef {
        match self {
            Self::EnsureWorkspace { host, .. } => host,
            other => &other.workspace().expect("workspace-scoped context").host,
        }
    }
    pub fn from_mutation(mutation: &Mutation) -> Self {
        match mutation {
            Mutation::CreateSession { workspace, .. } => Self::CreateSession {
                workspace: workspace.clone(),
            },
            Mutation::EnsureWorkspace { host, path } => Self::EnsureWorkspace {
                host: host.clone(),
                path: path.clone(),
            },
            Mutation::SendMessage { session, .. } => Self::SendMessage {
                session: session.clone(),
            },
            Mutation::Cancel { session } => Self::Cancel {
                session: session.clone(),
            },
            Mutation::SelectModel { session, .. } => Self::SelectModel {
                session: session.clone(),
            },
            Mutation::RespondPermission {
                session,
                request_id,
                ..
            } => Self::RespondPermission {
                session: session.clone(),
                request_id: request_id.clone(),
            },
            Mutation::RespondQuestion {
                session,
                request_id,
                ..
            } => Self::RespondQuestion {
                session: session.clone(),
                request_id: request_id.clone(),
            },
            Mutation::UploadAttachment { upload } => Self::UploadAttachment {
                session: upload.session.clone(),
            },
        }
    }
}

/// A private journal record and the safe local restore DTO. No prompt, title,
/// selected answer, attachment content, local file path or provider error text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JournalEntry {
    pub version: u32,
    pub connection: ConnectionRef,
    pub operation_id: OperationId,
    /// None means a retained ID imported without its original request. It can
    /// be inspected, but can never authorize dispatch or confirm changed args.
    pub request_digest: Option<String>,
    pub context: Option<OperationContext>,
    pub outcome: OperationOutcome,
}

pub struct JournalReservation {
    pub inserted: bool,
    pub entry: JournalEntry,
}

/// Implementations atomically reserve IDs and preserve immutable request binding.
/// A successful reserve/update must be durably committed before returning. Never
/// delete Unknown entries, recreate missing managed storage, or save request data.
#[async_trait]
pub trait OperationJournal: Send + Sync {
    async fn get(
        &self,
        connection: &ConnectionRef,
        id: &OperationId,
    ) -> Result<Option<JournalEntry>, AppError>;
    async fn list(&self, connection: &ConnectionRef) -> Result<Vec<JournalEntry>, AppError>;
    async fn reserve(&self, entry: &JournalEntry) -> Result<JournalReservation, AppError>;
    async fn update(&self, entry: &JournalEntry) -> Result<JournalEntry, AppError>;
}

pub fn journal_unavailable() -> AppError {
    AppError::domain(
        "remote_agent_journal_unavailable",
        "The local operation journal is unavailable; no new operation was dispatched",
    )
}
fn invalid_record() -> AppError {
    AppError::domain(
        "remote_agent_journal_invalid",
        "The local operation journal is invalid",
    )
}
pub(super) fn conflict() -> AppError {
    AppError::domain(
        "remote_agent_operation_conflict",
        "Operation ID was already used for a different request",
    )
}
pub fn unknown_outcome() -> OperationOutcome {
    OperationOutcome::Unknown {
        reason: UNCERTAIN.into(),
    }
}

/// Provider-controlled diagnostic strings can echo prompts or credentials. Keep
/// them out of disk and local restore DTOs while retaining typed result refs.
pub fn journal_outcome(outcome: &OperationOutcome) -> OperationOutcome {
    match outcome {
        OperationOutcome::Unknown { .. } => unknown_outcome(),
        OperationOutcome::Rejected { .. } => OperationOutcome::Rejected {
            code: "remote_agent_rejected".into(),
            message: "Remote provider rejected this operation".into(),
        },
        other => other.clone(),
    }
}

fn canonical(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(values) => {
            let mut values: Vec<_> = values.into_iter().collect();
            values.sort_by(|a, b| a.0.cmp(&b.0));
            serde_json::Value::Object(
                values
                    .into_iter()
                    .map(|(key, value)| (key, canonical(value)))
                    .collect(),
            )
        }
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.into_iter().map(canonical).collect())
        }
        other => other,
    }
}
impl JournalEntry {
    pub fn intent(request: &OperationRequest) -> Result<Self, AppError> {
        let raw = serde_json::to_vec(&canonical(serde_json::to_value(request)?))?;
        let request_digest = hex::encode(Sha256::digest(raw));
        let entry = Self {
            version: JOURNAL_VERSION,
            connection: request.mutation.host().connection.clone(),
            operation_id: request.operation_id.clone(),
            request_digest: Some(request_digest),
            context: Some(OperationContext::from_mutation(&request.mutation)),
            outcome: unknown_outcome(),
        };
        entry.validate()?;
        Ok(entry)
    }
    pub fn imported(connection: &ConnectionRef, id: &OperationId) -> Self {
        Self {
            version: JOURNAL_VERSION,
            connection: connection.clone(),
            operation_id: id.clone(),
            request_digest: None,
            context: None,
            outcome: unknown_outcome(),
        }
    }
    pub fn validate(&self) -> Result<(), AppError> {
        super::service::connection_ref(&self.connection)?;
        super::service::operation_id(&self.operation_id)?;
        if self.version != JOURNAL_VERSION || self.outcome != journal_outcome(&self.outcome) {
            return Err(invalid_record());
        }
        if let Some(digest) = &self.request_digest {
            if digest.len() != 64
                || !digest
                    .bytes()
                    .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
                || self.context.is_none()
            {
                return Err(invalid_record());
            }
        }
        if let Some(context) = &self.context {
            super::service::host_ref(context.host())?;
            if context.host().connection != self.connection {
                return Err(invalid_record());
            }
            if let Some(workspace) = context.workspace() {
                super::service::workspace_ref(workspace)?;
            }
            match context {
                OperationContext::CreateSession { .. } => {}
                OperationContext::EnsureWorkspace { path, .. } => {
                    super::service::workspace_path(path)?;
                }
                OperationContext::RespondPermission {
                    session,
                    request_id,
                }
                | OperationContext::RespondQuestion {
                    session,
                    request_id,
                } => {
                    super::service::session_ref(session)?;
                    super::service::identifier(request_id)?;
                }
                OperationContext::SendMessage { session }
                | OperationContext::Cancel { session }
                | OperationContext::SelectModel { session }
                | OperationContext::UploadAttachment { session } => {
                    super::service::session_ref(session)?
                }
            }
        }
        validate_context_outcome(&self.outcome, self.context.as_ref(), &self.connection)
    }
    pub(super) fn check_request(
        &self,
        request: &OperationRequest,
    ) -> Result<OperationOutcome, AppError> {
        self.validate()?;
        let proposed = Self::intent(request)?;
        if self.connection != proposed.connection || self.operation_id != proposed.operation_id {
            return Err(conflict());
        }
        if self
            .request_digest
            .as_ref()
            .is_some_and(|digest| Some(digest) != proposed.request_digest.as_ref())
        {
            return Err(conflict());
        }
        validate_context_outcome(&self.outcome, proposed.context.as_ref(), &self.connection)?;
        if self
            .context
            .as_ref()
            .is_some_and(|context| Some(context) != proposed.context.as_ref())
        {
            return Err(conflict());
        }
        Ok(if self.request_digest.is_some() {
            self.outcome.clone()
        } else {
            unknown_outcome()
        })
    }
    pub(super) fn with_outcome(&self, outcome: &OperationOutcome) -> Result<Self, AppError> {
        let mut updated = self.clone();
        updated.outcome = journal_outcome(outcome);
        if updated.context.is_none() {
            updated.context = match &updated.outcome {
                OperationOutcome::Succeeded {
                    result: OperationResult::SessionCreated { session },
                } => Some(OperationContext::CreateSession {
                    workspace: session.workspace.clone(),
                }),
                OperationOutcome::Succeeded {
                    result: OperationResult::WorkspaceEnsured { .. },
                } => None, // Host-level ensure needs its original path; never guess it.
                OperationOutcome::Succeeded {
                    result: OperationResult::AttachmentUploaded { attachment },
                } => Some(OperationContext::UploadAttachment {
                    session: attachment.session.clone(),
                }),
                OperationOutcome::Succeeded {
                    result: OperationResult::ModelSelected { session, .. },
                } => Some(OperationContext::SelectModel {
                    session: session.clone(),
                }),
                _ => None,
            };
        }
        updated.validate()?;
        Ok(updated)
    }
}

pub(super) fn validate_context_outcome(
    outcome: &OperationOutcome,
    context: Option<&OperationContext>,
    connection: &ConnectionRef,
) -> Result<(), AppError> {
    let OperationOutcome::Succeeded { result } = outcome else {
        return Ok(());
    };
    match result {
        OperationResult::SessionCreated { session } => {
            super::service::session_ref(session)?;
            if &session.workspace.host.connection != connection {
                return Err(super::service::boundary());
            }
            if let Some(context) = context {
                if !matches!(context, OperationContext::CreateSession { workspace } if workspace == &session.workspace)
                {
                    return Err(super::service::boundary());
                }
            }
        }
        OperationResult::WorkspaceEnsured { workspace } => {
            super::service::workspace_ref(workspace)?;
            if &workspace.host.connection != connection {
                return Err(super::service::boundary());
            }
            if let Some(context) = context {
                if !matches!(context, OperationContext::EnsureWorkspace { host, .. } if host == &workspace.host)
                {
                    return Err(super::service::boundary());
                }
            }
        }
        OperationResult::AttachmentUploaded { attachment } => {
            super::service::session_ref(&attachment.session)?;
            super::service::identifier(&attachment.path)?;
            if &attachment.session.workspace.host.connection != connection {
                return Err(super::service::boundary());
            }
            if let Some(context) = context {
                if !matches!(context, OperationContext::UploadAttachment { session } if session == &attachment.session)
                {
                    return Err(super::service::boundary());
                }
            }
        }
        OperationResult::ModelSelected { session, selected } => {
            super::service::session_ref(session)?;
            super::service::validate_model_selection(selected)?;
            if &session.workspace.host.connection != connection {
                return Err(super::service::boundary());
            }
            if let Some(context) = context {
                if !matches!(context, OperationContext::SelectModel { session: target } if target == session)
                {
                    return Err(super::service::boundary());
                }
            }
        }
        OperationResult::Acknowledged => {
            if matches!(
                context,
                Some(
                    OperationContext::CreateSession { .. }
                        | OperationContext::EnsureWorkspace { .. }
                        | OperationContext::UploadAttachment { .. }
                        | OperationContext::SelectModel { .. }
                )
            ) {
                return Err(invalid_record());
            }
        }
    }
    Ok(())
}

/// Storage-side immutable binding and monotone outcome checks, shared by SQLite
/// and the default in-memory implementation. Imported contexts may only narrow.
pub fn merge_journal_update(
    previous: &JournalEntry,
    proposed: &JournalEntry,
) -> Result<JournalEntry, AppError> {
    previous.validate()?;
    proposed.validate()?;
    if previous.connection != proposed.connection
        || previous.operation_id != proposed.operation_id
        || previous.request_digest != proposed.request_digest
        || previous
            .context
            .as_ref()
            .is_some_and(|context| Some(context) != proposed.context.as_ref())
    {
        return Err(conflict());
    }
    if matches!(
        previous.outcome,
        OperationOutcome::Succeeded { .. } | OperationOutcome::Rejected { .. }
    ) || matches!(
        (&previous.outcome, &proposed.outcome),
        (OperationOutcome::Accepted, OperationOutcome::Unknown { .. })
    ) {
        return Ok(previous.clone());
    }
    Ok(proposed.clone())
}

#[derive(Default)]
pub struct MemoryOperationJournal {
    entries: Mutex<HashMap<(ConnectionRef, OperationId), JournalEntry>>,
}
#[async_trait]
impl OperationJournal for MemoryOperationJournal {
    async fn get(
        &self,
        connection: &ConnectionRef,
        id: &OperationId,
    ) -> Result<Option<JournalEntry>, AppError> {
        Ok(self
            .entries
            .lock()
            .map_err(|_| journal_unavailable())?
            .get(&(connection.clone(), id.clone()))
            .cloned())
    }
    async fn list(&self, connection: &ConnectionRef) -> Result<Vec<JournalEntry>, AppError> {
        let mut entries: Vec<_> = self
            .entries
            .lock()
            .map_err(|_| journal_unavailable())?
            .values()
            .filter(|entry| &entry.connection == connection)
            .cloned()
            .collect();
        entries.sort_by(|a, b| a.operation_id.0.cmp(&b.operation_id.0));
        Ok(entries)
    }
    async fn reserve(&self, entry: &JournalEntry) -> Result<JournalReservation, AppError> {
        entry.validate()?;
        if !matches!(entry.outcome, OperationOutcome::Unknown { .. }) {
            return Err(invalid_record());
        }
        let mut entries = self.entries.lock().map_err(|_| journal_unavailable())?;
        let key = (entry.connection.clone(), entry.operation_id.clone());
        if let Some(previous) = entries.get(&key) {
            return Ok(JournalReservation {
                inserted: false,
                entry: previous.clone(),
            });
        }
        entries.insert(key, entry.clone());
        Ok(JournalReservation {
            inserted: true,
            entry: entry.clone(),
        })
    }
    async fn update(&self, entry: &JournalEntry) -> Result<JournalEntry, AppError> {
        let mut entries = self.entries.lock().map_err(|_| journal_unavailable())?;
        let key = (entry.connection.clone(), entry.operation_id.clone());
        let previous = entries.get(&key).ok_or_else(invalid_record)?;
        let updated = merge_journal_update(previous, entry)?;
        entries.insert(key, updated.clone());
        Ok(updated)
    }
}
