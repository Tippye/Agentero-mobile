//! Mocked wire vectors grounded in the pinned upstream Typert client and tests.
use super::*;
use crate::integration::dsh_remote::{
    gateway::WireRpc, protocol::Envelope, provider::DshRemoteProvider,
};
use std::sync::atomic::{AtomicBool, Ordering};

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/native-typert.json")).unwrap()
}
fn session() -> SessionRef {
    SessionRef {
        workspace: WorkspaceRef {
            host: HostRef {
                connection: ConnectionRef {
                    provider_id: DEFAULT_PROVIDER_ID.into(),
                    connection_id: "configured-account".into(),
                },
                host_id: "host".into(),
            },
            backend_id: "harness".into(),
            workspace_id: "native-workspace".into(),
        },
        session_id: "native-session".into(),
    }
}
#[derive(Clone)]
struct Request {
    id: String,
    method: String,
    params: Value,
    semantics: CallSemantics,
}
struct MockWire {
    events: broadcast::Sender<Envelope>,
    requests: StdMutex<Vec<Request>>,
    snapshot: StdMutex<Value>,
    remaining_ms: u64,
    lose_mutation: AtomicBool,
    uploaded: StdMutex<Vec<u8>>,
}
impl MockWire {
    fn new() -> Arc<Self> {
        Self::with_deadline(10000)
    }
    fn with_deadline(remaining_ms: u64) -> Arc<Self> {
        let (events, _) = broadcast::channel(256);
        Arc::new(Self {
            events,
            requests: StdMutex::new(Vec::new()),
            snapshot: StdMutex::new(fixture()["snapshot"].clone()),
            remaining_ms,
            lose_mutation: AtomicBool::new(false),
            uploaded: StdMutex::new(Vec::new()),
        })
    }
    fn frame(&self, stream: &str, value: Value) {
        let _ = self.events.send(Envelope::new("event", json!({"event":"harness.remote.frame","data":{"streamId":stream,"hasValue":true,"value":value}})));
    }
    fn calls(&self, endpoint: &str) -> Vec<Request> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.method == "harness.remote.call" && r.params["endpoint"] == endpoint)
            .cloned()
            .collect()
    }
    fn stream(&self, endpoint: &str) -> String {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|r| r.method == "harness.remote.stream.open" && r.params["endpoint"] == endpoint)
            .unwrap()
            .params["streamId"]
            .as_str()
            .unwrap()
            .into()
    }
}
#[async_trait]
impl WireRpc for MockWire {
    async fn request(
        &self,
        id: &str,
        method: &str,
        params: Value,
        semantics: CallSemantics,
        _budget: Duration,
    ) -> Result<Value> {
        self.requests.lock().unwrap().push(Request {
            id: id.into(),
            method: method.into(),
            params: params.clone(),
            semantics,
        });
        match method {
            "harness.remote.stream.open" => {
                let first = match params["endpoint"].as_str().unwrap() {
                    "$events" => {
                        json!({"type":"ready","clientId":"native-client-owner","host":{"sessionFormat":3}})
                    }
                    "workspace/follow" => fixture()["workspaceBaseline"].clone(),
                    "session/follow" => self.snapshot.lock().unwrap().clone(),
                    "userQuestions/attachWait" => json!({"remainingMs":self.remaining_ms}),
                    other => panic!("unexpected stream {other}"),
                };
                // Delivered before the open RPC settles, as allowed by upstream.
                self.frame(params["streamId"].as_str().unwrap(), first);
                Ok(json!({}))
            }
            "harness.remote.stream.close" => Ok(json!({})),
            "harness.remote.call" => {
                let endpoint = params["endpoint"].as_str().unwrap();
                if self.lose_mutation.load(Ordering::Acquire)
                    && semantics == CallSemantics::Mutation
                {
                    return Err(
                        RemoteError::new("RPC_TIMEOUT", "Lost native mutation response").unknown(),
                    );
                }
                let value = match endpoint {
                    "session/list" => fixture()["sessionCatalog"].clone(),
                    "session/page" => fixture()["olderPage"].clone(),
                    "session/create" => json!({"sessionId":"native-created-session"}),
                    "session/prompt" | "$events/result" => json!({}),
                    "session/cancel" => json!({"accepted":true}),
                    "userQuestions/answer" => json!(true),
                    other => panic!("unexpected unary endpoint {other}"),
                };
                // Namespace methods may return a business RpcResult inside the
                // outer Gateway result; both must preserve failures.
                Ok(json!({"ok":true,"value":{"ok":true,"value":value}}))
            }
            "workspace.upload.open" => Ok(json!({"uploadId":params["uploadId"],"chunkBytes":4})),
            "workspace.upload.chunk" => {
                use base64::Engine;
                let chunk = base64::engine::general_purpose::STANDARD
                    .decode(params["data"].as_str().unwrap())
                    .unwrap();
                let mut uploaded = self.uploaded.lock().unwrap();
                assert_eq!(params["offset"].as_u64().unwrap(), uploaded.len() as u64);
                uploaded.extend(chunk);
                Ok(json!({"uploadId":params["uploadId"],"receivedBytes":uploaded.len()}))
            }
            "workspace.upload.commit" => Ok(
                json!({"name":"paper.pdf","path":"./paper.pdf","bytes":self.uploaded.lock().unwrap().len()}),
            ),
            "workspace.upload.abort" => Ok(json!({"aborted":true})),
            other => panic!("unexpected wire method {other}"),
        }
    }
    fn events(&self) -> broadcast::Receiver<Envelope> {
        self.events.subscribe()
    }
    async fn closed(&self) {
        std::future::pending::<()>().await
    }
    fn is_closed(&self) -> bool {
        false
    }
}
async fn make_host(wire: Arc<MockWire>, upload: bool) -> Arc<NativeHost> {
    let mut capabilities = vec!["harness.remote.v3".into()];
    if upload {
        capabilities.push("workspace.upload.v1".into());
    }
    NativeHost::start(wire, capabilities, Arc::default())
        .await
        .unwrap()
}
fn operation(id: &str, mutation: Mutation) -> OperationRequest {
    OperationRequest {
        operation_id: id.into(),
        mutation,
    }
}
fn assert_call(wire: &MockWire, endpoint: &str, expected: &str) {
    assert_eq!(
        wire.calls(endpoint).last().unwrap().params,
        fixture()["expected"][expected]
    );
}

#[tokio::test]
async fn official_native_catalog_history_and_workspace_ownership() {
    let wire = MockWire::new();
    let host = make_host(wire.clone(), false).await;
    let session = session();
    let workspaces = host.workspace_rows().await.unwrap();
    assert_eq!(workspaces[0].workspace_id, "native-workspace");
    let sessions = host.catalog(&session.workspace).await.unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].session_id, "native-session");
    assert_call(&wire, "session/list", "sessionList");
    let snapshot = host.read(&session).await.unwrap();
    assert_eq!(snapshot.info.session, session);
    assert_eq!(snapshot.info.status, TaskStatus::Idle);
    assert!(snapshot.events.iter().any(|event| event.payload
        == EventPayload::Message {
            role: "user".into(),
            text: "Native prompt".into(),
            reasoning: None,
        }));
    assert!(snapshot.events.iter().any(|event| event.payload
        == EventPayload::Message {
            role: "assistant".into(),
            text: "Native response".into(),
            reasoning: None,
        }));
    assert_eq!(snapshot.cursor.unwrap(), model::cursor(&session, 4));
    assert_call(&wire, "session/page", "page");
    let mut other = session.clone();
    other.session_id = "unowned-session".into();
    assert_eq!(
        host.require_session(&other).await.err().unwrap().code,
        "SESSION_NOT_IN_WORKSPACE"
    );
    let mut foreign = session.workspace.clone();
    foreign.backend_id = "codex".into();
    assert_eq!(
        host.catalog(&foreign).await.err().unwrap().code,
        "FEATURE_NOT_SUPPORTED"
    );
}

#[tokio::test]
async fn exact_create_prompt_cancel_requests_and_unknown_no_fallback() {
    let wire = MockWire::new();
    let host = make_host(wire.clone(), false).await;
    let session = session();
    let created = DshRemoteProvider::mutate_on_host(
        &host,
        &operation(
            "operation-create-01",
            Mutation::CreateSession {
                workspace: session.workspace.clone(),
                title: None,
            },
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        created,
        OperationOutcome::Succeeded {
            result: OperationResult::SessionCreated {
                session: SessionRef {
                    session_id: "native-created-session".into(),
                    ..session.clone()
                }
            }
        }
    );
    assert_call(&wire, "session/create", "create");
    DshRemoteProvider::mutate_on_host(
        &host,
        &operation(
            "operation-prompt-01",
            Mutation::SendMessage {
                session: session.clone(),
                text: "Do the work".into(),
                attachments: vec![],
            },
        ),
    )
    .await
    .unwrap();
    assert_call(&wire, "session/prompt", "prompt");
    let prompt = wire.calls("session/prompt").pop().unwrap();
    assert_eq!(prompt.id, "operation-prompt-01");
    assert_eq!(prompt.semantics, CallSemantics::Mutation);
    assert_eq!(
        DshRemoteProvider::mutate_on_host(
            &host,
            &operation(
                "operation-cancel-01",
                Mutation::Cancel {
                    session: session.clone()
                }
            )
        )
        .await
        .unwrap(),
        OperationOutcome::Accepted
    );
    assert_call(&wire, "session/cancel", "cancel");
    let count = wire.calls("session/create").len();
    assert_eq!(
        DshRemoteProvider::mutate_on_host(
            &host,
            &operation(
                "operation-title-01",
                Mutation::CreateSession {
                    workspace: session.workspace.clone(),
                    title: Some("Unsupported title".into())
                }
            )
        )
        .await
        .unwrap_err()
        .code,
        "FEATURE_NOT_SUPPORTED"
    );
    assert_eq!(wire.calls("session/create").len(), count);
    wire.lose_mutation.store(true, Ordering::Release);
    let error = DshRemoteProvider::mutate_on_host(
        &host,
        &operation("operation-unknown01", Mutation::Cancel { session }),
    )
    .await
    .unwrap_err();
    assert!(error.outcome_unknown);
    assert_eq!(wire.calls("session/cancel").len(), 2);
    assert!(!wire
        .requests
        .lock()
        .unwrap()
        .iter()
        .any(|r| r.method.starts_with("harness.api.") || r.params["endpoint"] == "sync.from"));
}

#[tokio::test]
async fn approvals_bind_native_owner_and_cannot_be_replayed_or_escalated() {
    let wire = MockWire::new();
    let host = make_host(wire.clone(), false).await;
    let session = session();
    host.route_interaction(fixture()["approval"].clone())
        .await
        .unwrap();
    let pending = host.pending_events(&session).await.unwrap();
    assert!(matches!(
        pending[0].payload,
        EventPayload::PermissionRequest { .. }
    ));
    assert_eq!(
        host.respond(
            "operation-badoption",
            &session,
            "native-approval",
            Some("allowed-always"),
            None
        )
        .await
        .unwrap_err()
        .code,
        "INVALID_MESSAGE"
    );
    host.respond(
        "operation-approval1",
        &session,
        "native-approval",
        Some("allowed-once"),
        None,
    )
    .await
    .unwrap();
    assert_call(&wire, "$events/result", "approval");
    assert_eq!(
        host.respond(
            "operation-approval2",
            &session,
            "native-approval",
            Some("allowed-once"),
            None
        )
        .await
        .unwrap_err()
        .code,
        "PERMISSION_NOT_PENDING"
    );
    assert_eq!(wire.calls("$events/result").len(), 1);
    let second = make_host(MockWire::new(), false).await;
    assert_eq!(
        second
            .respond(
                "operation-otherpeer",
                &session,
                "native-approval",
                Some("allowed-once"),
                None
            )
            .await
            .unwrap_err()
            .code,
        "PERMISSION_NOT_PENDING"
    );
}

#[tokio::test]
async fn blocking_timed_claim_and_continued_question_wire_contracts() {
    let wire = MockWire::new();
    let host = make_host(wire.clone(), false).await;
    let session = session();
    host.route_interaction(fixture()["question"].clone())
        .await
        .unwrap();
    let answer = QuestionAnswer {
        question_id: "q1".into(),
        selected: vec!["Fast".into()],
        custom: None,
    };
    host.respond(
        "operation-question1",
        &session,
        "native-question",
        None,
        Some(&[answer]),
    )
    .await
    .unwrap();
    assert_call(&wire, "$events/result", "question");
    host.route_interaction(fixture()["timedQuestion"].clone())
        .await
        .unwrap();
    let claim = wire
        .requests
        .lock()
        .unwrap()
        .iter()
        .find(|r| r.params["endpoint"] == "userQuestions/attachWait")
        .unwrap()
        .clone();
    assert_eq!(
        claim.params["payload"],
        json!({"args":{"agentId":"native-session","callId":"timed-call"}})
    );
    host.respond(
        "operation-timed-001",
        &session,
        "native-timed",
        None,
        Some(&[QuestionAnswer {
            question_id: "q1".into(),
            selected: vec!["Yes".into()],
            custom: None,
        }]),
    )
    .await
    .unwrap();
    assert_eq!(
        wire.calls("$events/result").last().unwrap().params["payload"]["args"]["eventId"],
        "native-timed"
    );
    host.apply_question_projection(&session, Some(&fixture()["continuedProjection"]))
        .await
        .unwrap();
    let request_id = host
        .pending_events(&session)
        .await
        .unwrap()
        .into_iter()
        .find_map(|event| match event.payload {
            EventPayload::QuestionRequest { request_id, .. } => Some(request_id),
            _ => None,
        })
        .unwrap();
    host.respond(
        "operation-continued",
        &session,
        &request_id,
        None,
        Some(&[QuestionAnswer {
            question_id: "q1".into(),
            selected: vec!["Continue".into()],
            custom: None,
        }]),
    )
    .await
    .unwrap();
    assert_call(&wire, "userQuestions/answer", "continuedAnswer");
    assert_eq!(
        host.respond(
            "operation-again-001",
            &session,
            &request_id,
            None,
            Some(&[])
        )
        .await
        .unwrap_err()
        .code,
        "PERMISSION_NOT_PENDING"
    );
}

#[tokio::test]
async fn expired_timed_question_settles_once_without_answer_replay() {
    let wire = MockWire::with_deadline(0);
    let host = make_host(wire.clone(), false).await;
    let session = session();
    host.route_interaction(fixture()["timedQuestion"].clone())
        .await
        .unwrap();
    assert_eq!(
        host.respond(
            "operation-expired01",
            &session,
            "native-timed",
            None,
            Some(&[QuestionAnswer {
                question_id: "q1".into(),
                selected: vec!["Yes".into()],
                custom: None
            }])
        )
        .await
        .unwrap_err()
        .code,
        "PERMISSION_NOT_PENDING"
    );
    host.expire_questions().await;
    host.expire_questions().await;
    assert_eq!(wire.calls("$events/result").len(), 1);
    assert_eq!(
        wire.calls("$events/result")[0].params["payload"]["args"]["outcome"]["error"]["code"],
        "ASK_TIMED_OUT"
    );
}

#[tokio::test]
async fn upload_requires_native_grant_and_uses_dedicated_bounded_protocol() {
    let wire = MockWire::new();
    let host = make_host(wire.clone(), true).await;
    let session = session();
    let upload = AttachmentUpload {
        session,
        local_path: "native-file-grant".into(),
        file_name: "paper.pdf".into(),
        mime_type: "application/pdf".into(),
        size_bytes: 7,
    };
    assert_eq!(
        host.upload("operation-upload-01", &upload)
            .await
            .unwrap_err()
            .code,
        "PERMISSION_DENIED"
    );
    let mut file = tempfile::tempfile().unwrap();
    use std::io::Write;
    file.write_all(b"payload").unwrap();
    host.grants.grant(upload.local_path.clone(), file).unwrap();
    let result = host.upload("operation-upload-02", &upload).await.unwrap();
    assert_eq!(result.path, "./paper.pdf");
    assert_eq!(result.size_bytes, 7);
    assert_eq!(&*wire.uploaded.lock().unwrap(), b"payload");
    {
        let calls = wire.requests.lock().unwrap();
        let opened = calls
            .iter()
            .find(|r| r.method == "workspace.upload.open")
            .unwrap();
        assert_eq!(opened.params["name"], "paper.pdf");
        assert_eq!(opened.params["totalBytes"], 7);
        let upload_id = opened.params["uploadId"].as_str().unwrap();
        assert!(uuid::Uuid::parse_str(upload_id).is_ok());
        for call in calls
            .iter()
            .filter(|r| r.method.starts_with("workspace.upload."))
        {
            assert_eq!(call.params["uploadId"], upload_id);
            assert_eq!(call.params["sessionId"], "native-session");
        }
        assert_eq!(
            calls
                .iter()
                .filter(|r| r.method == "workspace.upload.chunk")
                .map(|r| r.params["data"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["cGF5bA==", "b2Fk"]
        );
    }
    assert_eq!(
        host.upload("operation-upload-03", &upload)
            .await
            .unwrap_err()
            .code,
        "PERMISSION_DENIED"
    );
    let unsupported = make_host(MockWire::new(), false).await;
    assert_eq!(
        unsupported
            .upload("operation-upload-04", &upload)
            .await
            .unwrap_err()
            .code,
        "FEATURE_NOT_SUPPORTED"
    );
}

#[test]
fn question_answer_validation_enforces_custom_policy_and_explicit_skip() {
    let question = Question {
        id: "q1".into(),
        prompt: "Pick".into(),
        options: vec!["Yes".into()],
        multiple: false,
        allows_custom: false,
    };
    let skip = QuestionAnswer {
        question_id: "q1".into(),
        selected: vec![],
        custom: None,
    };
    assert!(validate_answers(std::slice::from_ref(&question), std::slice::from_ref(&skip)).is_ok());
    let custom = QuestionAnswer {
        custom: Some("Other".into()),
        ..skip.clone()
    };
    assert!(validate_answers(
        std::slice::from_ref(&question),
        std::slice::from_ref(&custom)
    )
    .is_err());
    let allowed = Question {
        allows_custom: true,
        ..question
    };
    assert!(validate_answers(
        std::slice::from_ref(&allowed),
        std::slice::from_ref(&custom)
    )
    .is_ok());
    assert!(validate_answers(&[allowed], &[]).is_err());
}

#[tokio::test]
async fn subscription_bridges_official_snapshot_and_preserves_event_cursor_identity() {
    let wire = MockWire::new();
    let host = make_host(wire.clone(), false).await;
    let session = session();
    let after = model::cursor(&session, 1);
    let mut subscription = host.subscribe(&session, Some(&after)).await.unwrap();
    let mut ids = HashSet::new();
    for _ in 0..4 {
        let event = subscription.next().await.unwrap().unwrap();
        assert_eq!(event.session, session);
        ids.insert(event.source_id);
    }
    assert!(ids.contains("seq:3"));
    assert!(!ids.contains("seq:1"));
    let stream = wire.stream("session/follow");
    wire.frame(&stream, json!({"type":"event","event":{"type":"assistant/message","seq":6,"time":1700000000600i64,"data":{"message":{"id":"native-live","role":"assistant","content":[{"type":"text","text":"Live native response"}]}}}}));
    let event = subscription.next().await.unwrap().unwrap();
    assert_eq!(event.source_id, "seq:6");
    assert_eq!(event.cursor.unwrap(), model::cursor(&session, 6));
    subscription.cancel().await.unwrap();
    assert!(subscription.next().await.unwrap().is_none());
    let bad = EventCursor {
        stream_id: "other-session".into(),
        value: "4".into(),
    };
    assert!(host.subscribe(&session, Some(&bad)).await.is_err());
}

#[tokio::test]
async fn native_projection_keeps_human_messages_reasoning_and_tool_rows() {
    let session = session();
    // Injected model-facing context is not a human turn and must not render.
    let injected = json!({"type":"user/message","seq":10,"data":{"message":{"id":"ctx","role":"user","source":{"kind":"plugin"},"content":[{"type":"text","text":"Current runtime context…"}]}}});
    assert!(model::events(&session, &injected).unwrap().is_empty());
    // Assistant reasoning rides along with the reply instead of a raw bubble.
    let reply = json!({"type":"assistant/message","seq":11,"data":{"message":{"id":"a1","role":"assistant","content":[{"type":"reasoning","text":"Thinking it through"},{"type":"text","text":"The answer"}]}}});
    let events = model::events(&session, &reply).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].payload,
        EventPayload::Message {
            role: "assistant".into(),
            text: "The answer".into(),
            reasoning: Some("Thinking it through".into()),
        }
    );
    // Call and result share one correlation ID so the UI folds them together.
    let call = json!({"type":"tool/call","seq":12,"data":{"callId":"call-1","name":"terminal","arguments":"{\"command\":\"ls\"}"}});
    let result = json!({"type":"tool/result","seq":13,"data":{"message":{"source":{"callId":"call-1"},"content":[{"type":"tool-result","content":[{"type":"text","text":"file-a\nfile-b"}]}]}}});
    let call_events = model::events(&session, &call).unwrap();
    let result_events = model::events(&session, &result).unwrap();
    assert_eq!(call_events[0].source_id, "tool:call-1");
    assert_eq!(result_events[0].source_id, "tool:call-1");
    assert_eq!(
        call_events[0].payload,
        EventPayload::Tool {
            call_id: "call-1".into(),
            name: "terminal".into(),
            state: "running".into(),
            arguments: Some("{\"command\":\"ls\"}".into()),
            output: None,
        }
    );
    assert_eq!(
        result_events[0].payload,
        EventPayload::Tool {
            call_id: "call-1".into(),
            name: "Tool".into(),
            state: "finished".into(),
            arguments: None,
            output: Some("file-a\nfile-b".into()),
        }
    );
    // An errored result is a failed row, never a silent success.
    let failed = json!({"type":"tool/result","seq":14,"data":{"callId":"call-2","name":"edit","isError":true,"message":{"content":[{"type":"text","text":"denied"}]}}});
    let failed_events = model::events(&session, &failed).unwrap();
    assert!(matches!(
        &failed_events[0].payload,
        EventPayload::Tool { state, .. } if state == "failed"
    ));
}
