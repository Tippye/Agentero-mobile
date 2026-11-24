use super::*;
use crate::integration::dsh_remote::{gateway::WireRpc, protocol::Envelope};
use serde_json::Value;
use tokio::sync::broadcast;

struct LocalWire {
    rpc: RpcClient,
    events: broadcast::Sender<Envelope>,
}
#[async_trait]
impl WireRpc for LocalWire {
    async fn request(
        &self,
        _: &str,
        method: &str,
        params: Value,
        _: CallSemantics,
        _: Duration,
    ) -> Result<Value> {
        match method {
            "harness.remote.stream.open" => {
                assert_eq!(params["endpoint"], "$events");
                let _ = self.events.send(Envelope::new("event", json!({"event":"harness.remote.frame","data":{"streamId":params["streamId"],"hasValue":true,"value":{"type":"ready","clientId":"network-fixture","host":{"sessionFormat":3}}}})));
                Ok(json!({}))
            }
            "harness.remote.stream.close" => Ok(json!({})),
            _ => panic!("network hint attempted a new remote request: {method}"),
        }
    }
    fn events(&self) -> broadcast::Receiver<Envelope> {
        self.events.subscribe()
    }
    async fn closed(&self) {
        self.rpc.wait_closed().await;
    }
    fn is_closed(&self) -> bool {
        self.rpc.is_closed()
    }
}

async fn configured(
    provider: &DshRemoteProvider,
    id: &str,
    connected: bool,
    rpc: RpcClient,
) -> Arc<Mutex<Connection>> {
    let identity = DeviceIdentity::generate("network-client".into()).unwrap();
    provider
        .configure(
            id.into(),
            DshConnectionConfig {
                api: ServerApi::new("http://127.0.0.1:9", Duration::from_secs(1)).unwrap(),
                identity,
                access_token: "unchanged-fixture-access-token".into(),
                pinned_hosts: HashMap::from([("selected-host".into(), "unchanged-pin".into())]),
            },
        )
        .await
        .unwrap();
    let reference = ConnectionRef {
        provider_id: DEFAULT_PROVIDER_ID.into(),
        connection_id: id.into(),
    };
    let connection = provider.connection(&reference).await.unwrap();
    let (events, _) = broadcast::channel(256);
    let native = NativeHost::start(
        Arc::new(LocalWire {
            rpc: rpc.clone(),
            events,
        }),
        vec!["harness.remote.v3".into()],
        Arc::default(),
    )
    .await
    .unwrap();
    let mut state = connection.lock().await;
    state.connected = connected;
    state
        .hosts
        .insert("selected-host".into(), HostRuntime { rpc, native });
    state.receipts.insert(
        "uncertain-operation".into(),
        Receipt {
            fingerprint: "unchanged-digest".into(),
            outcome: OperationOutcome::Unknown {
                reason: "reply lost".into(),
            },
        },
    );
    drop(state);
    connection
}

#[tokio::test]
async fn reselecting_the_active_trusted_host_does_not_dial_again() {
    let provider = DshRemoteProvider::default();
    let (rpc, _) = RpcClient::pending_test_client();
    let state = configured(&provider, "configured", true, rpc.clone()).await;
    let host = HostRef {
        connection: ConnectionRef {
            provider_id: DEFAULT_PROVIDER_ID.into(),
            connection_id: "configured".into(),
        },
        host_id: "selected-host".into(),
    };
    provider
        .select_host(&host, "unchanged-pin".into())
        .await
        .unwrap();
    provider.resume(&host.connection).await.unwrap();
    assert!(!rpc.is_closed());
    assert!(
        state.lock().await.hosts["selected-host"]
            .native
            .is_healthy()
            .await
    );
    rpc.close().await;
}

#[tokio::test]
async fn network_hint_closes_only_live_rpcs_preserves_authority_and_unknown_receipts() {
    let provider = DshRemoteProvider::default();
    let (active_rpc, admitted) = RpcClient::pending_test_client();
    let active = configured(&provider, "active", true, active_rpc.clone()).await;
    let (inactive_rpc, _) = RpcClient::pending_test_client();
    let inactive = configured(&provider, "disconnected", false, inactive_rpc.clone()).await;
    let grants = active.lock().await.grants.clone();
    let mutation_rpc = active_rpc.clone();
    let pending = tokio::spawn(async move {
        mutation_rpc
            .call_with_id(
                "pending-mutation",
                "harness.remote.call",
                json!({}),
                CallSemantics::Mutation,
                Duration::from_secs(5),
            )
            .await
    });
    admitted.await.unwrap();
    provider.network_changed().await;
    assert!(active_rpc.is_closed());
    assert!(!inactive_rpc.is_closed());
    assert!(pending.await.unwrap().unwrap_err().outcome_unknown);
    let state = active.lock().await;
    assert!(state.connected);
    assert_eq!(state.hosts.len(), 1);
    assert!(state.hosts.contains_key("selected-host"));
    assert!(!state.hosts["selected-host"].native.is_healthy().await);
    assert!(Arc::ptr_eq(&grants, &state.grants));
    assert_eq!(state.config.access_token, "unchanged-fixture-access-token");
    assert_eq!(state.config.pinned_hosts["selected-host"], "unchanged-pin");
    assert_eq!(
        state.receipts["uncertain-operation"].fingerprint,
        "unchanged-digest"
    );
    assert!(matches!(
        state.receipts["uncertain-operation"].outcome,
        OperationOutcome::Unknown { .. }
    ));
    drop(state);
    assert!(!inactive.lock().await.connected);
    // Repeated advisory hints do not create a replacement transport or account.
    provider.network_changed().await;
    assert_eq!(provider.connections.lock().await.len(), 2);
    inactive_rpc.close().await;
}
