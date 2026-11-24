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
                        json!({"type":"ready","clientId":"native-client-owner","host":{"sessionFormat":3,"home":"/home/native"}})
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
                    "session/modelCatalog" => fixture()["modelCatalog"].clone(),
                    "workspace/create" => {
                        json!({"workspace":{"workspaceId":"native-workspace","path":params["payload"]["args"]["request"]["path"]},"created":false})
                    }
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
    // The packed reasoning row surfaces as one concatenated delta.
    assert!(snapshot
        .events
        .iter()
        .any(|event| event.source_id == "chunk:3"
            && event.payload
                == EventPayload::AssistantChunk {
                    text_delta: None,
                    reasoning_delta: Some("Thinking natively".into()),
                }));
    assert_eq!(snapshot.cursor.unwrap(), model::cursor(&session, 5));
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
    assert_eq!(
        DshRemoteProvider::mutate_on_host(
            &host,
            &operation(
                "operation-paper-prompt",
                Mutation::SendMessage {
                    session: upload.session.clone(),
                    text: "Summarize the attached paper.".into(),
                    attachments: vec![result],
                },
            ),
        )
        .await
        .unwrap(),
        OperationOutcome::Accepted
    );
    let prompt = wire.calls("session/prompt").pop().unwrap();
    assert_eq!(
        prompt.params["payload"]["args"]["request"]["content"],
        json!([{
            "type": "text",
            "text": "Summarize the attached paper.\n\nAttached files in this session's workspace:\n- ./paper.pdf\n"
        }])
    );
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
    for _ in 0..5 {
        let event = subscription.next().await.unwrap().unwrap();
        assert_eq!(event.session, session);
        ids.insert(event.source_id);
    }
    assert!(ids.contains("chunk:3"));
    assert!(ids.contains("seq:4"));
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

#[test]
fn tolerant_projection_skips_unknown_shapes_without_failing_the_read() {
    let session = session();
    // Flat user/message shape (data.source + data.content) is a human turn.
    let flat = json!({"type":"user/message","seq":20,"data":{"source":{"kind":"user"},"content":[{"type":"text","text":"Flat prompt"}]}});
    let events = model::events(&session, &flat).unwrap();
    assert_eq!(
        events[0].payload,
        EventPayload::Message {
            role: "user".into(),
            text: "Flat prompt".into(),
            reasoning: None,
        }
    );
    // Flat injected context is still filtered out.
    let flat_plugin = json!({"type":"user/message","seq":21,"data":{"source":{"kind":"plugin"},"content":[{"type":"text","text":"ctx"}]}});
    assert!(model::events(&session, &flat_plugin).unwrap().is_empty());
    // Missing content / missing callId / unknown types are skips, not errors.
    for native in [
        json!({"type":"user/message","seq":22,"data":{"source":{"kind":"user"}}}),
        json!({"type":"assistant/message","seq":23,"data":{}}),
        json!({"type":"tool/call","seq":24,"data":{"name":"terminal"}}),
        json!({"type":"tool/result","seq":25,"data":{}}),
        json!({"type":"session/title-llm-request","seq":26,"data":{"messages":[]}}),
        json!({"type":"chunkrow/tool-call-chunks","seq":27,"data":{"args":["{}"]}}),
    ] {
        assert!(model::events(&session, &native).unwrap().is_empty());
    }
    // A text chunk row joins its texts into one delta with a stable source ID.
    let row = json!({"type":"chunkrow/text-chunks","seq":28,"time":100,"data":{"turn":1,"step":1,"dt":[5],"texts":["hel","lo"]}});
    let events = model::events(&session, &row).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].source_id, "chunk:28");
    assert_eq!(
        events[0].payload,
        EventPayload::AssistantChunk {
            text_delta: Some("hello".into()),
            reasoning_delta: None,
        }
    );
}

#[tokio::test]
async fn read_expands_active_assistant_stream_after_durable_history() {
    let wire = MockWire::new();
    *wire.snapshot.lock().unwrap() = fixture()["streamSnapshot"].clone();
    let host = make_host(wire.clone(), false).await;
    let session = session();
    let snapshot = host.read(&session).await.unwrap();
    let chunks: Vec<_> = snapshot
        .events
        .iter()
        .filter(|event| matches!(event.payload, EventPayload::AssistantChunk { .. }))
        .collect();
    // nextIndex=4 caps expansion: raw reasoning chunk, two packed texts, one
    // tool-call member (skipped); the 5th raw chunk beyond the cap is dropped.
    assert_eq!(chunks.len(), 3);
    assert_eq!(chunks[0].source_id, "stream:att-live:0");
    assert_eq!(
        chunks[0].payload,
        EventPayload::AssistantChunk {
            text_delta: None,
            reasoning_delta: Some("Considering ".into()),
        }
    );
    assert_eq!(chunks[1].source_id, "stream:att-live:1");
    assert_eq!(chunks[2].source_id, "stream:att-live:2");
    assert_eq!(
        chunks[2].payload,
        EventPayload::AssistantChunk {
            text_delta: Some("reply".into()),
            reasoning_delta: None,
        }
    );
    // Durable events come first; transient chunks carry no durable cursor.
    let durable = snapshot
        .events
        .iter()
        .position(|event| event.source_id == "seq:4")
        .unwrap();
    let first_chunk = snapshot
        .events
        .iter()
        .position(|event| event.source_id == "stream:att-live:0")
        .unwrap();
    assert!(durable < first_chunk);
    assert!(chunks.iter().all(|event| event.cursor.is_none()));
    assert_eq!(snapshot.cursor.unwrap(), model::cursor(&session, 4));
    // v3 follow requests opt into assistantStream.
    let open = wire
        .requests
        .lock()
        .unwrap()
        .iter()
        .find(|r| {
            r.method == "harness.remote.stream.open" && r.params["endpoint"] == "session/follow"
        })
        .unwrap()
        .clone();
    assert_eq!(
        open.params["payload"]["args"]["request"]["assistantStream"],
        true
    );
}

#[tokio::test]
async fn subscription_projects_live_assistant_stream_frames_without_cursor_advance() {
    let wire = MockWire::new();
    *wire.snapshot.lock().unwrap() = fixture()["streamSnapshot"].clone();
    let host = make_host(wire.clone(), false).await;
    let session = session();
    let mut subscription = host.subscribe(&session, None).await.unwrap();
    // Drain baseline: turn/start, 3 stream chunks, baseline status.
    let mut baseline = Vec::new();
    for _ in 0..5 {
        baseline.push(subscription.next().await.unwrap().unwrap());
    }
    assert!(baseline.iter().any(|e| e.source_id == "stream:att-live:0"));
    let stream = wire.stream("session/follow");
    // The active attempt continues at index 4 (nextIndex was 4).
    wire.frame(&stream, json!({"type":"assistant-stream","frame":{"type":"chunk","attemptId":"att-live","revision":6,"index":4,"time":1700000000450i64,"chunk":{"type":"text-delta","index":1,"text":" more"}}}));
    let event = subscription.next().await.unwrap().unwrap();
    assert_eq!(event.source_id, "stream:att-live:4");
    assert_eq!(
        event.payload,
        EventPayload::AssistantChunk {
            text_delta: Some(" more".into()),
            reasoning_delta: None,
        }
    );
    assert!(event.cursor.is_none());
    // A replayed index is deduplicated by the subscription's seen set upstream;
    // here the adapter itself drops indices below next_index.
    wire.frame(&stream, json!({"type":"assistant-stream","frame":{"type":"chunk","attemptId":"att-live","revision":7,"index":4,"time":1700000000460i64,"chunk":{"type":"text-delta","index":1,"text":"dup"}}}));
    // End clears the attempt; a new start rebinds indices from zero.
    wire.frame(&stream, json!({"type":"assistant-stream","frame":{"type":"end","attemptId":"att-live","revision":8,"index":5,"outcome":{"kind":"committed","eventType":"assistant/message","seq":5}}}));
    wire.frame(&stream, json!({"type":"assistant-stream","frame":{"type":"start","attemptId":"att-next","revision":9,"startedAfterSeq":5,"turn":2,"step":2}}));
    wire.frame(&stream, json!({"type":"assistant-stream","frame":{"type":"chunk","attemptId":"att-next","revision":10,"index":0,"time":1700000000470i64,"chunk":{"type":"reasoning-delta","index":0,"text":"retry thought"}}}));
    let event = subscription.next().await.unwrap().unwrap();
    assert_eq!(event.source_id, "stream:att-next:0");
    assert_eq!(
        event.payload,
        EventPayload::AssistantChunk {
            text_delta: None,
            reasoning_delta: Some("retry thought".into()),
        }
    );
    // Durable gating is untouched: the next durable event still advances seq.
    wire.frame(&stream, json!({"type":"event","event":{"type":"turn/end","seq":6,"time":1700000000480i64,"data":{"reason":"completed"}}}));
    let event = subscription.next().await.unwrap().unwrap();
    assert_eq!(event.source_id, "seq:6");
    assert_eq!(event.cursor.unwrap(), model::cursor(&session, 6));
    subscription.cancel().await.unwrap();
}

#[tokio::test]
async fn host_model_catalog_projects_default_without_session_row() {
    let wire = MockWire::new();
    let host = make_host(wire.clone(), false).await;
    let models = host.session_model_catalog().await.unwrap().unwrap();
    assert_eq!(models.current.provider, "deepseek");
    assert_eq!(models.current.model, "deepseek-chat");
    assert!(models.routable);
    assert_eq!(models.groups.len(), 1);
    assert_call(&wire, "session/modelCatalog", "modelCatalog");
    // Session-scoped projection still layers the row's modelSelection on top.
    let session = session();
    let scoped = host.session_models(&session).await.unwrap().unwrap();
    assert_eq!(scoped.current.model, "deepseek-chat");
}

#[tokio::test]
async fn ensure_workspace_reuses_matching_path_and_creates_otherwise() {
    let wire = MockWire::new();
    let host = make_host(wire.clone(), false).await;
    let reference = session().workspace.host;
    // Existing baseline path: pure read, no workspace/create write.
    let reused = host
        .ensure_workspace("operation-ensure-01", &reference, "/remote/project")
        .await
        .unwrap();
    assert_eq!(reused.workspace_id.0, "native-workspace");
    assert_eq!(reused.backend_id.0, "harness");
    assert!(wire.calls("workspace/create").is_empty());
    // Unknown path: workspace/create rides the caller's journaled operation ID.
    let created = host
        .ensure_workspace("operation-ensure-02", &reference, "/remote/paper-project")
        .await
        .unwrap();
    assert_eq!(created.workspace_id.0, "native-workspace");
    assert_call(&wire, "workspace/create", "workspaceCreate");
    let call = wire.calls("workspace/create").pop().unwrap();
    assert_eq!(call.id, "operation-ensure-02");
    assert_eq!(call.semantics, CallSemantics::Mutation);
    // Invalid paths are refused before any wire traffic.
    for path in ["relative/dir", "/", "C:\\", "/bad\0path"] {
        assert!(host
            .ensure_workspace("operation-ensure-03", &reference, path)
            .await
            .is_err());
    }
    assert_eq!(wire.calls("workspace/create").len(), 1);
    // The full mutation flow returns the typed WorkspaceEnsured result.
    let outcome = DshRemoteProvider::mutate_on_host(
        &host,
        &operation(
            "operation-ensure-04",
            Mutation::EnsureWorkspace {
                host: reference.clone(),
                path: "/remote/project".into(),
            },
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        outcome,
        OperationOutcome::Succeeded {
            result: OperationResult::WorkspaceEnsured {
                workspace: WorkspaceRef {
                    host: reference,
                    backend_id: "harness".into(),
                    workspace_id: "native-workspace".into(),
                }
            }
        }
    );
}

#[tokio::test]
async fn unassigned_virtual_workspace_lists_only_trusted_papers_sessions() {
    let wire = MockWire::new();
    let host = make_host(wire.clone(), false).await;
    assert_eq!(host.home.as_deref(), Some("/home/native"));
    assert_eq!(
        host.papers_cwd().as_deref(),
        Some("/home/native/Agentero/Papers")
    );
    let mut workspace = session().workspace;
    workspace.workspace_id = "agentero:unassigned".into();
    let rows = host.catalog(&workspace).await.unwrap();
    // Only the root session at the exact Papers cwd; owned, subagent and
    // unrelated-cwd sessions never appear in the virtual scope.
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].session_id, "paper-session");
    let paper = SessionRef {
        workspace: workspace.clone(),
        session_id: "paper-session".into(),
    };
    assert!(host.require_session(&paper).await.is_ok());
    let stray = SessionRef {
        workspace: workspace.clone(),
        session_id: "stray-session".into(),
    };
    assert_eq!(
        host.require_session(&stray).await.err().unwrap().code,
        "SESSION_NOT_IN_WORKSPACE"
    );
    // The official baseline itself never gains the virtual row.
    assert!(host
        .workspace_rows()
        .await
        .unwrap()
        .iter()
        .all(|row| row.workspace_id != "agentero:unassigned"));
    // CreateSession on the virtual scope sends cwd only — true unassigned.
    let created = DshRemoteProvider::mutate_on_host(
        &host,
        &operation(
            "operation-unassign1",
            Mutation::CreateSession {
                workspace: workspace.clone(),
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
                    workspace,
                    session_id: "native-created-session".into(),
                }
            }
        }
    );
    assert_call(&wire, "session/create", "createUnassigned");
}

#[test]
fn windows_host_paths_compare_normalized_and_unix_stays_case_sensitive() {
    assert!(model::same_host_path(
        "C:\\Users\\Payne\\Agentero\\Papers",
        "c:/users/payne/agentero/papers/"
    ));
    assert!(!model::same_host_path(
        "/home/Native/Agentero/Papers",
        "/home/native/Agentero/Papers"
    ));
    assert!(model::same_host_path(
        "/home/native/Agentero/Papers/",
        "/home/native/Agentero/Papers"
    ));
}

#[tokio::test]
async fn read_projects_context_pressure_preferring_projected_tokens() {
    let wire = MockWire::new();
    *wire.snapshot.lock().unwrap() = fixture()["contextSnapshot"].clone();
    let host = make_host(wire.clone(), false).await;
    let snapshot = host.read(&session()).await.unwrap();
    assert_eq!(
        snapshot.context_usage,
        Some(ContextUsage {
            used: 52000,
            size: 128000
        })
    );
    // Absence stays honest: the default fixture has no contextPressure.
    let plain = make_host(MockWire::new(), false).await;
    assert_eq!(plain.read(&session()).await.unwrap().context_usage, None);
    // ACP-style {used,size} without a window is not displayable.
    let acp = json!({"projections":{"values":{"contextPressure":{"used":10,"size":0}}}});
    assert_eq!(model::context_usage(&acp), None);
    let legacy = json!({"projections":{"values":{"contextPressure":{"used":10,"size":200}}}});
    assert_eq!(
        model::context_usage(&legacy),
        Some(ContextUsage {
            used: 10,
            size: 200
        })
    );
}
