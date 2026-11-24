use super::*;
use crate::core::error::AppError;
use async_trait::async_trait;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

fn connection(provider: &str, instance: &str) -> ConnectionRef {
    ConnectionRef {
        provider_id: provider.into(),
        connection_id: instance.into(),
    }
}

fn workspace(connection: &ConnectionRef, backend: &str) -> WorkspaceRef {
    WorkspaceRef {
        host: HostRef {
            connection: connection.clone(),
            host_id: "same-host".into(),
        },
        backend_id: backend.into(),
        workspace_id: "same-workspace".into(),
    }
}

fn session(connection: &ConnectionRef, backend: &str) -> SessionRef {
    SessionRef {
        workspace: workspace(connection, backend),
        session_id: "same-session".into(),
    }
}

fn send(session: &SessionRef) -> OperationRequest {
    OperationRequest {
        operation_id: "operation-send-0001".into(),
        mutation: Mutation::SendMessage {
            session: session.clone(),
            text: "Do the work".into(),
            attachments: vec![],
        },
    }
}

fn event(session: &SessionRef, stream: &str, id: &str) -> RemoteEvent {
    RemoteEvent {
        session: session.clone(),
        source_stream_id: stream.into(),
        source_id: id.into(),
        cursor: Some(EventCursor {
            stream_id: stream.into(),
            value: id.into(),
        }),
        payload: EventPayload::Message {
            role: "assistant".into(),
            text: id.into(),
            reasoning: None,
        },
    }
}

fn capabilities() -> Capabilities {
    Capabilities {
        operations: vec![
            Capability::ListHosts,
            Capability::ListWorkspaces,
            Capability::ListSessions,
            Capability::CreateSession,
            Capability::ReadSession,
            Capability::SendMessage,
            Capability::Cancel,
            Capability::Subscribe,
            Capability::RespondPermission,
            Capability::RespondQuestion,
            Capability::UploadAttachment,
            Capability::OperationReceipts,
            Capability::Resume,
        ],
        authentication: vec![AuthenticationMethod::ExistingCredential],
        backends: vec!["harness".into(), "acp".into()],
        attachments: Some(AttachmentLimits {
            max_bytes: 20,
            mime_types: vec!["application/pdf".into()],
        }),
    }
}

struct FakeProvider {
    id: ProviderId,
    capabilities: Mutex<Capabilities>,
    connects: AtomicUsize,
    disconnects: AtomicUsize,
    resumes: AtomicUsize,
    network_hints: AtomicUsize,
    dispatches: AtomicUsize,
    reads: AtomicUsize,
    crossed_namespace: AtomicBool,
    fail_execution: AtomicBool,
    pending_execution: AtomicBool,
    entered: Notify,
    receipt: Mutex<ReceiptLookup>,
    events: Mutex<Vec<RemoteEvent>>,
    subscribed_cursor: Mutex<Option<EventCursor>>,
    cancellations: Arc<AtomicUsize>,
}

impl FakeProvider {
    fn new(id: &str) -> Self {
        Self {
            id: id.into(),
            capabilities: Mutex::new(capabilities()),
            connects: AtomicUsize::new(0),
            disconnects: AtomicUsize::new(0),
            resumes: AtomicUsize::new(0),
            network_hints: AtomicUsize::new(0),
            dispatches: AtomicUsize::new(0),
            reads: AtomicUsize::new(0),
            crossed_namespace: AtomicBool::new(false),
            fail_execution: AtomicBool::new(false),
            pending_execution: AtomicBool::new(false),
            entered: Notify::new(),
            receipt: Mutex::new(ReceiptLookup::NotFound),
            events: Mutex::new(vec![]),
            subscribed_cursor: Mutex::new(None),
            cancellations: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn info(&self, connection: &ConnectionRef) -> ConnectionInfo {
        ConnectionInfo {
            connection: connection.clone(),
            capabilities: self.capabilities.lock().unwrap().clone(),
        }
    }
}

struct FakeSubscription {
    events: VecDeque<RemoteEvent>,
    cancellations: Arc<AtomicUsize>,
}

#[async_trait]
impl RemoteAgentSubscription for FakeSubscription {
    async fn next(&mut self) -> Result<Option<RemoteEvent>, AppError> {
        Ok(self.events.pop_front())
    }
    async fn cancel(&mut self) -> Result<(), AppError> {
        self.cancellations.fetch_add(1, Ordering::SeqCst);
        self.events.clear();
        Ok(())
    }
}

#[async_trait]
impl RemoteAgentProvider for FakeProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            provider_id: self.id.clone(),
            display_name: "Test adapter".into(),
            contract_version: CONTRACT_VERSION,
            config_version: 1,
        }
    }

    async fn connect(&self, request: &ConnectRequest) -> Result<ConnectionInfo, AppError> {
        self.connects.fetch_add(1, Ordering::SeqCst);
        Ok(self.info(&request.connection))
    }

    async fn disconnect(&self, _: &ConnectionRef) -> Result<(), AppError> {
        self.disconnects.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn network_changed(&self) {
        self.network_hints.fetch_add(1, Ordering::SeqCst);
    }

    async fn resume(&self, connection: &ConnectionRef) -> Result<ConnectionInfo, AppError> {
        self.resumes.fetch_add(1, Ordering::SeqCst);
        Ok(self.info(connection))
    }

    async fn list_hosts(&self, connection: &ConnectionRef) -> Result<Vec<HostInfo>, AppError> {
        let mut host = workspace(connection, "harness").host;
        if self.crossed_namespace.load(Ordering::SeqCst) {
            host.connection.connection_id = "other-instance".into();
        }
        Ok(vec![HostInfo {
            host,
            name: "Host".into(),
            online: Some(true),
            pinned: true,
            home: None,
        }])
    }

    async fn list_workspaces(&self, host: &HostRef) -> Result<Vec<WorkspaceInfo>, AppError> {
        let mut result = workspace(&host.connection, "harness");
        result.host = host.clone();
        Ok(vec![WorkspaceInfo {
            workspace: result,
            name: "Workspace".into(),
            path: Some("/remote/workspace".into()),
        }])
    }

    async fn list_sessions(&self, workspace: &WorkspaceRef) -> Result<Vec<SessionInfo>, AppError> {
        Ok(vec![SessionInfo {
            session: SessionRef {
                workspace: workspace.clone(),
                session_id: "same-session".into(),
            },
            title: "Task".into(),
            status: TaskStatus::Idle,
        }])
    }

    async fn read_session(&self, target: &SessionRef) -> Result<SessionSnapshot, AppError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let mut target = target.clone();
        if self.crossed_namespace.load(Ordering::SeqCst) {
            target.workspace.backend_id = "acp".into();
        }
        let first = event(&target, "epoch-1", "event-1");
        Ok(SessionSnapshot {
            info: SessionInfo {
                session: target,
                title: "Task".into(),
                status: TaskStatus::WaitingForPermission,
            },
            events: vec![first.clone(), first.clone()],
            cursor: first.cursor,
            context_usage: None,
        })
    }

    async fn subscribe(
        &self,
        _: &SessionRef,
        after: Option<&EventCursor>,
    ) -> Result<Box<dyn RemoteAgentSubscription>, AppError> {
        *self.subscribed_cursor.lock().unwrap() = after.cloned();
        Ok(Box::new(FakeSubscription {
            events: self.events.lock().unwrap().clone().into(),
            cancellations: self.cancellations.clone(),
        }))
    }

    async fn execute(&self, request: &OperationRequest) -> Result<OperationOutcome, AppError> {
        self.dispatches.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_one();
        // Make concurrent service calls contend while dispatch is in flight.
        tokio::task::yield_now().await;
        if self.pending_execution.load(Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        if self.fail_execution.load(Ordering::SeqCst) {
            return Err(AppError::message("Transport lost after dispatch"));
        }
        let result = match &request.mutation {
            Mutation::CreateSession { workspace, .. } => {
                let mut workspace = workspace.clone();
                if self.crossed_namespace.load(Ordering::SeqCst) {
                    workspace.backend_id = "acp".into();
                }
                OperationResult::SessionCreated {
                    session: SessionRef {
                        workspace,
                        session_id: "created".into(),
                    },
                }
            }
            Mutation::UploadAttachment { upload } => OperationResult::AttachmentUploaded {
                attachment: RemoteAttachment {
                    session: upload.session.clone(),
                    path: "uploads/paper.pdf".into(),
                    size_bytes: upload.size_bytes,
                },
            },
            _ => OperationResult::Acknowledged,
        };
        Ok(OperationOutcome::Succeeded { result })
    }

    async fn lookup_receipt(
        &self,
        _: &ConnectionRef,
        _: &OperationId,
    ) -> Result<ReceiptLookup, AppError> {
        Ok(self.receipt.lock().unwrap().clone())
    }
}

async fn setup() -> (Arc<RemoteAgentService>, Arc<FakeProvider>, ConnectionRef) {
    let service = Arc::new(RemoteAgentService::default());
    let provider = Arc::new(FakeProvider::new(DEFAULT_PROVIDER_ID));
    let connection = connection(DEFAULT_PROVIDER_ID, "account-one");
    service.register(provider.clone()).unwrap();
    service
        .connect(&ConnectRequest {
            connection: connection.clone(),
            config_version: 1,
        })
        .await
        .unwrap();
    (service, provider, connection)
}

fn assert_code<T>(result: Result<T, AppError>, code: &str) {
    match result {
        Err(error) => assert_eq!(error.code(), code),
        Ok(_) => panic!("Expected {code}"),
    }
}

#[tokio::test]
async fn empty_registry_is_honest_and_duplicate_registration_is_rejected() {
    let service = RemoteAgentService::default();
    assert_eq!(
        service.default_provider().unwrap(),
        ProviderId::from(DEFAULT_PROVIDER_ID)
    );
    assert!(service.providers().unwrap().is_empty());
    assert_code(
        service
            .connect(&ConnectRequest {
                connection: connection(DEFAULT_PROVIDER_ID, "one"),
                config_version: 1,
            })
            .await,
        "remote_agent_provider_not_found",
    );
    let provider = Arc::new(FakeProvider::new("custom"));
    service.register(provider.clone()).unwrap();
    assert_code(
        service.register(provider),
        "remote_agent_duplicate_provider",
    );
    service.set_default_provider(&"custom".into()).unwrap();
    assert_eq!(
        service.default_provider().unwrap(),
        ProviderId::from("custom")
    );
    assert_code(
        service.set_default_provider(&"missing".into()),
        "remote_agent_provider_not_found",
    );
}

#[tokio::test]
async fn connection_is_shared_and_versions_and_invalid_ids_fail_before_adapter() {
    let (service, provider, connection) = setup().await;
    let request = ConnectRequest {
        connection: connection.clone(),
        config_version: 1,
    };
    let (first, second) = tokio::join!(service.connect(&request), service.connect(&request));
    assert_eq!(first.unwrap(), second.unwrap());
    assert_eq!(provider.connects.load(Ordering::SeqCst), 1);
    assert_code(
        service
            .connect(&ConnectRequest {
                config_version: 2,
                ..request.clone()
            })
            .await,
        "remote_agent_invalid",
    );
    let mut invalid = request;
    invalid.connection.connection_id = "\n".into();
    assert_code(service.connect(&invalid).await, "remote_agent_invalid");
    assert_eq!(provider.connects.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn provider_connection_and_backend_namespaces_do_not_alias() {
    let (service, first, first_connection) = setup().await;
    let second = Arc::new(FakeProvider::new("second"));
    service.register(second.clone()).unwrap();
    let second_connection = connection("second", "account-one");
    let another_instance = connection(DEFAULT_PROVIDER_ID, "account-two");
    for connection in [&second_connection, &another_instance] {
        service
            .connect(&ConnectRequest {
                connection: connection.clone(),
                config_version: 1,
            })
            .await
            .unwrap();
    }
    for (connection, backend) in [
        (&first_connection, "harness"),
        (&second_connection, "harness"),
        (&another_instance, "harness"),
        (&first_connection, "acp"),
    ] {
        let mut request = send(&session(connection, backend));
        if backend == "acp" {
            request.operation_id = "operation-acp-0001".into();
        }
        service.execute(&request).await.unwrap();
    }
    assert_eq!(first.dispatches.load(Ordering::SeqCst), 3);
    assert_eq!(second.dispatches.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn namespaces_are_checked_on_provider_outputs_and_stream_events() {
    let (service, provider, connection) = setup().await;
    provider.crossed_namespace.store(true, Ordering::SeqCst);
    assert_code(
        service.list_hosts(&connection).await,
        "remote_agent_namespace",
    );
    let target = session(&connection, "harness");
    assert_code(
        service.read_session(&target).await,
        "remote_agent_namespace",
    );
    provider.crossed_namespace.store(false, Ordering::SeqCst);
    let foreign = session(&connection, "acp");
    *provider.events.lock().unwrap() = vec![event(&foreign, "epoch-1", "wrong-session")];
    let mut recovered = service.recover_session(&target).await.unwrap();
    assert_code(
        recovered.subscription.next().await,
        "remote_agent_namespace",
    );
    assert_eq!(provider.cancellations.load(Ordering::SeqCst), 1);
    assert!(recovered.subscription.next().await.unwrap().is_none());
}

#[tokio::test]
async fn unsupported_capabilities_are_rejected_before_dispatch_and_recovery_reads() {
    let (service, provider, connection) = setup().await;
    provider.capabilities.lock().unwrap().operations = vec![Capability::Resume];
    service.resume(&connection).await.unwrap();
    let target = session(&connection, "harness");
    let mutations = vec![
        Mutation::CreateSession {
            workspace: target.workspace.clone(),
            title: None,
        },
        send(&target).mutation,
        Mutation::Cancel {
            session: target.clone(),
        },
        Mutation::RespondPermission {
            session: target.clone(),
            request_id: "request".into(),
            option_id: "allow".into(),
        },
        Mutation::RespondQuestion {
            session: target.clone(),
            request_id: "request".into(),
            answers: vec![],
        },
        Mutation::UploadAttachment {
            upload: upload(&target, 20),
        },
    ];
    for mutation in mutations {
        assert_code(
            service
                .execute(&OperationRequest {
                    operation_id: "unsupported-op-0001".into(),
                    mutation,
                })
                .await,
            "remote_agent_unsupported",
        );
    }
    assert_code(
        service.list_hosts(&connection).await,
        "remote_agent_unsupported",
    );
    assert_code(
        service.list_workspaces(&target.workspace.host).await,
        "remote_agent_unsupported",
    );
    assert_code(
        service.list_sessions(&target.workspace).await,
        "remote_agent_unsupported",
    );
    assert_code(
        service.read_session(&target).await,
        "remote_agent_unsupported",
    );
    assert_code(
        service.recover_session(&target).await,
        "remote_agent_unsupported",
    );
    assert_code(
        service
            .lookup_receipt(&connection, &"receipt-check-0001".into())
            .await,
        "remote_agent_unsupported",
    );
    assert_eq!(provider.dispatches.load(Ordering::SeqCst), 0);
    assert_eq!(provider.reads.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn unavailable_backend_and_cross_session_attachment_are_rejected() {
    let (service, provider, connection) = setup().await;
    assert_code(
        service.execute(&send(&session(&connection, "codex"))).await,
        "remote_agent_backend_unsupported",
    );
    let target = session(&connection, "harness");
    let mut request = send(&target);
    if let Mutation::SendMessage { attachments, .. } = &mut request.mutation {
        attachments.push(RemoteAttachment {
            session: session(&connection, "acp"),
            path: "file.pdf".into(),
            size_bytes: 1,
        });
    }
    assert_code(service.execute(&request).await, "remote_agent_namespace");
    assert_eq!(provider.dispatches.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn concurrent_duplicate_operation_dispatches_once_and_conflicting_reuse_fails() {
    let (service, provider, connection) = setup().await;
    let request = send(&session(&connection, "harness"));
    let (first, second) = tokio::join!(service.execute(&request), service.execute(&request));
    assert_eq!(first.unwrap(), second.unwrap());
    assert_eq!(provider.dispatches.load(Ordering::SeqCst), 1);
    let mut changed = request.clone();
    changed.mutation = Mutation::Cancel {
        session: session(&connection, "harness"),
    };
    assert_code(
        service.execute(&changed).await,
        "remote_agent_operation_conflict",
    );
    service.disconnect(&connection).await.unwrap();
    assert!(matches!(
        service.execute(&request).await.unwrap(),
        OperationOutcome::Succeeded { .. }
    ));
    assert_eq!(provider.dispatches.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn network_hint_preserves_disconnect_unknown_receipts_and_never_resumes() {
    let (service, provider, connection) = setup().await;
    let unused = Arc::new(FakeProvider::new("unused-provider"));
    service.register(unused.clone()).unwrap();
    provider.fail_execution.store(true, Ordering::SeqCst);
    let request = send(&session(&connection, "harness"));
    let before = service.execute(&request).await.unwrap();
    assert!(matches!(before, OperationOutcome::Unknown { .. }));
    service.network_changed().await;
    assert_eq!(service.execute(&request).await.unwrap(), before);
    assert_eq!(provider.dispatches.load(Ordering::SeqCst), 1);
    service.disconnect(&connection).await.unwrap();
    service.network_changed().await;
    assert!(service
        .read_session(&session(&connection, "harness"))
        .await
        .is_err());
    assert_eq!(provider.connects.load(Ordering::SeqCst), 1);
    assert_eq!(provider.resumes.load(Ordering::SeqCst), 0);
    assert_eq!(provider.network_hints.load(Ordering::SeqCst), 2);
    assert_eq!(unused.network_hints.load(Ordering::SeqCst), 2);
    assert_eq!(unused.connects.load(Ordering::SeqCst), 0);
    assert_eq!(unused.resumes.load(Ordering::SeqCst), 0);
    assert_eq!(unused.dispatches.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn uncertain_write_is_not_resent_by_resume_or_missing_receipt() {
    let (service, provider, connection) = setup().await;
    provider.fail_execution.store(true, Ordering::SeqCst);
    let request = send(&session(&connection, "harness"));
    assert!(matches!(
        service.execute(&request).await.unwrap(),
        OperationOutcome::Unknown { .. }
    ));
    service.resume(&connection).await.unwrap();
    assert_eq!(
        service
            .lookup_receipt(&connection, &request.operation_id)
            .await
            .unwrap(),
        ReceiptLookup::NotFound
    );
    assert!(matches!(
        service.execute(&request).await.unwrap(),
        OperationOutcome::Unknown { .. }
    ));
    assert_eq!(provider.dispatches.load(Ordering::SeqCst), 1);
    *provider.receipt.lock().unwrap() = ReceiptLookup::Found(OperationOutcome::Accepted);
    service
        .lookup_receipt(&connection, &request.operation_id)
        .await
        .unwrap();
    assert_eq!(
        service.execute(&request).await.unwrap(),
        OperationOutcome::Accepted
    );
    assert_eq!(provider.dispatches.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn receipt_lookup_after_restart_blocks_redispatch_even_when_not_found() {
    let (service, provider, connection) = setup().await;
    let request = send(&session(&connection, "harness"));
    assert_eq!(
        service
            .lookup_receipt(&connection, &request.operation_id)
            .await
            .unwrap(),
        ReceiptLookup::NotFound
    );
    assert!(matches!(
        service.execute(&request).await.unwrap(),
        OperationOutcome::Unknown { .. }
    ));
    assert_eq!(provider.dispatches.load(Ordering::SeqCst), 0);

    let retained_id: OperationId = "retained-create-0001".into();
    *provider.receipt.lock().unwrap() = ReceiptLookup::Found(OperationOutcome::Succeeded {
        result: OperationResult::SessionCreated {
            session: session(&connection, "acp"),
        },
    });
    service
        .lookup_receipt(&connection, &retained_id)
        .await
        .unwrap();
    assert_code(
        service
            .execute(&OperationRequest {
                operation_id: retained_id,
                mutation: Mutation::CreateSession {
                    workspace: workspace(&connection, "harness"),
                    title: None,
                },
            })
            .await,
        "remote_agent_namespace",
    );
    assert_eq!(provider.dispatches.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn invalid_write_result_and_receipt_preserve_uncertainty() {
    let (service, provider, connection) = setup().await;
    let target = session(&connection, "harness");
    let request = OperationRequest {
        operation_id: "create-session-0001".into(),
        mutation: Mutation::CreateSession {
            workspace: target.workspace.clone(),
            title: None,
        },
    };
    provider.crossed_namespace.store(true, Ordering::SeqCst);
    assert!(matches!(
        service.execute(&request).await.unwrap(),
        OperationOutcome::Unknown { .. }
    ));
    *provider.receipt.lock().unwrap() = ReceiptLookup::Found(OperationOutcome::Succeeded {
        result: OperationResult::SessionCreated {
            session: session(&connection, "acp"),
        },
    });
    assert_code(
        service
            .lookup_receipt(&connection, &request.operation_id)
            .await,
        "remote_agent_namespace",
    );
    assert!(matches!(
        service.execute(&request).await.unwrap(),
        OperationOutcome::Unknown { .. }
    ));
    assert_eq!(provider.dispatches.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn unsupported_receipt_lookup_still_remembers_retained_operation_ids() {
    let (service, provider, connection) = setup().await;
    provider
        .capabilities
        .lock()
        .unwrap()
        .operations
        .retain(|cap| *cap != Capability::OperationReceipts);
    service.resume(&connection).await.unwrap();
    let request = send(&session(&connection, "harness"));
    assert_code(
        service
            .lookup_receipt(&connection, &request.operation_id)
            .await,
        "remote_agent_unsupported",
    );
    assert!(matches!(
        service.execute(&request).await.unwrap(),
        OperationOutcome::Unknown { .. }
    ));
    assert_eq!(provider.dispatches.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn cancelled_local_future_retains_unknown_outcome_and_never_redispatches() {
    let (service, provider, connection) = setup().await;
    provider.pending_execution.store(true, Ordering::SeqCst);
    let request = send(&session(&connection, "harness"));
    let task_service = service.clone();
    let task_request = request.clone();
    let task = tokio::spawn(async move { task_service.execute(&task_request).await });
    provider.entered.notified().await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(matches!(
        service
            .local_outcome(&connection, &request.operation_id)
            .await
            .unwrap(),
        Some(OperationOutcome::Unknown { .. })
    ));
    assert!(matches!(
        service.execute(&request).await.unwrap(),
        OperationOutcome::Unknown { .. }
    ));
    assert_eq!(provider.dispatches.load(Ordering::SeqCst), 1);
}

fn upload(session: &SessionRef, size_bytes: u64) -> AttachmentUpload {
    AttachmentUpload {
        session: session.clone(),
        local_path: "/local/paper.pdf".into(),
        file_name: "paper.pdf".into(),
        mime_type: "application/pdf".into(),
        size_bytes,
    }
}

#[tokio::test]
async fn attachment_limit_is_inclusive_and_preflight_failure_does_not_consume_id() {
    let (service, provider, connection) = setup().await;
    let target = session(&connection, "harness");
    let mut request = OperationRequest {
        operation_id: "upload-operation-0001".into(),
        mutation: Mutation::UploadAttachment {
            upload: upload(&target, 21),
        },
    };
    assert_code(
        service.execute(&request).await,
        "remote_agent_attachment_too_large",
    );
    let mut wrong_type = upload(&target, 20);
    wrong_type.mime_type = "text/html".into();
    request.mutation = Mutation::UploadAttachment { upload: wrong_type };
    assert_code(
        service.execute(&request).await,
        "remote_agent_attachment_type",
    );
    request.mutation = Mutation::UploadAttachment {
        upload: upload(&target, 20),
    };
    assert!(matches!(
        service.execute(&request).await.unwrap(),
        OperationOutcome::Succeeded {
            result: OperationResult::AttachmentUploaded { .. }
        }
    ));
    assert_eq!(provider.dispatches.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn recovery_deduplicates_history_overlap_without_sharing_window_cursors() {
    let (service, provider, connection) = setup().await;
    let target = session(&connection, "harness");
    *provider.events.lock().unwrap() = vec![
        event(&target, "epoch-1", "event-1"),
        event(&target, "epoch-1", "event-2"),
        event(&target, "epoch-1", "event-2"),
        event(&target, "epoch-2", "event-2"),
    ];
    let mut first = service.recover_session(&target).await.unwrap();
    let mut second = service.recover_session(&target).await.unwrap();
    assert_eq!(first.snapshot.events.len(), 1);
    assert_eq!(first.snapshot, second.snapshot);
    assert_eq!(
        *provider.subscribed_cursor.lock().unwrap(),
        first.snapshot.cursor
    );
    assert_eq!(
        first.subscription.next().await.unwrap().unwrap().source_id,
        "event-2"
    );
    assert_eq!(
        second.subscription.next().await.unwrap().unwrap().source_id,
        "event-2"
    );
    assert_eq!(
        first
            .subscription
            .next()
            .await
            .unwrap()
            .unwrap()
            .source_stream_id,
        "epoch-2"
    );
    assert!(first.subscription.next().await.unwrap().is_none());
    second.subscription.cancel().await.unwrap();
    assert!(second.subscription.next().await.unwrap().is_none());
    assert_eq!(provider.cancellations.load(Ordering::SeqCst), 1);
    assert_eq!(provider.dispatches.load(Ordering::SeqCst), 0);
}

#[test]
fn wire_contract_keeps_namespaces_and_explicit_unknown_and_question_fields() {
    let target = session(&connection("custom", "one"), "acp");
    let request = OperationRequest {
        operation_id: "question-answer-0001".into(),
        mutation: Mutation::RespondQuestion {
            session: target.clone(),
            request_id: "question-request".into(),
            answers: vec![QuestionAnswer {
                question_id: "q1".into(),
                selected: vec!["choice".into()],
                custom: None,
            }],
        },
    };
    let value = serde_json::to_value(&request).unwrap();
    assert_eq!(value["mutation"]["type"], "respondQuestion");
    assert_eq!(
        value["mutation"]["session"]["workspace"]["host"]["connection"]["providerId"],
        "custom"
    );
    assert_eq!(value["mutation"]["answers"][0]["questionId"], "q1");
    assert_eq!(
        serde_json::from_value::<OperationRequest>(value).unwrap(),
        request
    );
    assert_eq!(
        serde_json::to_value(OperationOutcome::Unknown {
            reason: "lost".into()
        })
        .unwrap()["status"],
        "unknown"
    );
}

#[tokio::test]
async fn ensure_workspace_is_host_scoped_journaled_and_path_bound() {
    let (service, provider, connection) = setup().await;
    provider
        .capabilities
        .lock()
        .unwrap()
        .operations
        .push(Capability::CreateSession);
    service.resume(&connection).await.unwrap();
    let host = workspace(&connection, "harness").host;
    let request = OperationRequest {
        operation_id: "ensure-workspace-0001".into(),
        mutation: Mutation::EnsureWorkspace {
            host: host.clone(),
            path: "/remote/paper-project".into(),
        },
    };
    // The journaled intent binds the host-level context and path digest.
    let entry = JournalEntry::intent(&request).unwrap();
    assert_eq!(entry.connection, connection);
    assert!(matches!(
        entry.context,
        Some(OperationContext::EnsureWorkspace { ref path, .. }) if path == "/remote/paper-project"
    ));
    // FakeProvider acknowledges generically; EnsureWorkspace requires its typed
    // result, so a bare Acknowledged is conservatively Unknown.
    assert!(matches!(
        service.execute(&request).await.unwrap(),
        OperationOutcome::Unknown { .. }
    ));
    assert_eq!(provider.dispatches.load(Ordering::SeqCst), 1);
    // The journaled digest binds the path: a different directory conflicts.
    let mut changed = request.clone();
    changed.mutation = Mutation::EnsureWorkspace {
        host: host.clone(),
        path: "/remote/other-project".into(),
    };
    assert_code(
        service.execute(&changed).await,
        "remote_agent_operation_conflict",
    );
    // Relative, root and control-character paths are refused before dispatch.
    for (index, path) in ["relative/dir", "/", "C:\\", "/bad\0path"]
        .into_iter()
        .enumerate()
    {
        let invalid = OperationRequest {
            operation_id: format!("ensure-invalid-{index:04}").as_str().into(),
            mutation: Mutation::EnsureWorkspace {
                host: host.clone(),
                path: path.into(),
            },
        };
        assert_code(service.execute(&invalid).await, "remote_agent_invalid");
    }
    assert_eq!(provider.dispatches.load(Ordering::SeqCst), 1);
    // A typed WorkspaceEnsured result settles the original operation.
    *provider.receipt.lock().unwrap() = ReceiptLookup::Found(OperationOutcome::Succeeded {
        result: OperationResult::WorkspaceEnsured {
            workspace: workspace(&connection, "harness"),
        },
    });
    service
        .lookup_receipt(&connection, &request.operation_id)
        .await
        .unwrap();
    assert!(matches!(
        service.execute(&request).await.unwrap(),
        OperationOutcome::Succeeded {
            result: OperationResult::WorkspaceEnsured { .. }
        }
    ));
    assert_eq!(provider.dispatches.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn host_models_requires_harness_backend_and_select_model_capability() {
    let (service, provider, connection) = setup().await;
    let host = workspace(&connection, "harness").host;
    // The base fixture has no SelectModel: the catalog gate returns None.
    assert!(service.host_models(&host).await.unwrap().is_none());
    provider
        .capabilities
        .lock()
        .unwrap()
        .operations
        .push(Capability::SelectModel);
    service.resume(&connection).await.unwrap();
    // FakeProvider keeps the trait default: no catalog on this route.
    assert!(service.host_models(&host).await.unwrap().is_none());
    provider.capabilities.lock().unwrap().backends = vec!["acp".into()];
    service.resume(&connection).await.unwrap();
    assert_code(
        service.host_models(&host).await,
        "remote_agent_backend_unsupported",
    );
}
