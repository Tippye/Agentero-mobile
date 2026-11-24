use super::*;
use crate::integration::{
    credentials::CredentialBackend,
    dsh_remote::{api::ServerApi, crypto::DeviceIdentity, protocol::encode_base64},
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex as StdMutex,
    },
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Default)]
struct Memory {
    value: StdMutex<Option<String>>,
    fault: StdMutex<Option<CredentialError>>,
    fail_remove: StdMutex<bool>,
    fail_write_at: StdMutex<Option<usize>>,
    writes: AtomicUsize,
    removes: AtomicUsize,
}
impl CredentialBackend for Memory {
    fn read(&self, _: &str) -> Result<Option<String>, CredentialError> {
        if let Some(error) = *self.fault.lock().unwrap() {
            return Err(error);
        }
        Ok(self.value.lock().unwrap().clone())
    }
    fn write(&self, _: &str, raw: &str) -> Result<(), CredentialError> {
        let count = self.writes.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some(error) = *self.fault.lock().unwrap() {
            return Err(error);
        }
        if *self.fail_write_at.lock().unwrap() == Some(count) {
            return Err(CredentialError::Unavailable);
        }
        *self.value.lock().unwrap() = Some(raw.into());
        Ok(())
    }
    fn remove(&self, _: &str) -> Result<(), CredentialError> {
        self.removes.fetch_add(1, Ordering::SeqCst);
        if *self.fail_remove.lock().unwrap() {
            return Err(CredentialError::Locked);
        }
        if let Some(error) = *self.fault.lock().unwrap() {
            return Err(error);
        }
        *self.value.lock().unwrap() = None;
        Ok(())
    }
}
fn fixture(origin: &str) -> LaunchAccount {
    let now = now_ms().unwrap();
    let connection = ConnectionRef {
        provider_id: DEFAULT_PROVIDER_ID.into(),
        connection_id: ConnectionId(uuid::Uuid::new_v4().to_string()),
    };
    LaunchAccount {
        api: ServerApi::new(origin, Duration::from_secs(2)).unwrap(),
        identity: DeviceIdentity::restore(
            uuid::Uuid::new_v4().to_string(),
            &encode_base64(&[7; 32]),
        )
        .unwrap(),
        credentials: crate::integration::dsh_remote::api::TokenPair {
            access_token: "access-token-fixture".into(),
            access_token_expires_at: now + 3_600_000,
            refresh_token: "refresh-token-fixture".into(),
            refresh_token_expires_at: now + 86_400_000,
        },
        refresh_pending: false,
        pins: HashMap::from([("host-1".into(), encode_base64(&[9; 32]))]),
        view: RemoteAgentAccount {
            account: "reader@example.test".into(),
            server: origin.into(),
            expires_at: now + 3_600_000,
            persistence: RemoteCredentialPersistence::SystemStore,
            login_provider: Some("github".into()),
            info: ConnectionInfo {
                connection,
                capabilities: Capabilities {
                    operations: vec![],
                    authentication: vec![],
                    backends: vec![],
                    attachments: None,
                },
            },
        },
    }
}
fn service() -> (RemoteAgentService, Arc<DshRemoteProvider>) {
    let service = RemoteAgentService::default();
    let provider = Arc::new(DshRemoteProvider::default());
    service.register(provider.clone()).unwrap();
    (service, provider)
}
fn stored(backend: &Memory) -> Value {
    serde_json::from_str(backend.value.lock().unwrap().as_ref().unwrap()).unwrap()
}

#[tokio::test]
async fn restart_hydrates_the_same_device_connection_and_pins_without_passwords() {
    let backend = Arc::new(Memory::default());
    let store = CredentialStore::from_backend(backend.clone());
    let original = fixture("https://remote.example/");
    persist_account(&original, Some(&store)).await.unwrap();
    let loaded = load_account(&store, now_ms().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded.identity.device_id, original.identity.device_id);
    assert_eq!(loaded.identity.public_key, original.identity.public_key);
    assert_eq!(loaded.view.info.connection, original.view.info.connection);
    assert_eq!(loaded.pins, original.pins);
    assert!(stored(&backend).get("password").is_none());
    assert_eq!(
        loaded.credentials.refresh_token,
        original.credentials.refresh_token
    );
}

#[tokio::test]
async fn missing_locked_unavailable_corrupt_expired_and_pending_are_distinct() {
    let (service, provider) = service();
    let backend = Arc::new(Memory::default());
    let store = CredentialStore::from_backend(backend.clone());
    let accounts = RemoteAgentAccounts::with_store(Some(store.clone()));
    accounts.ensure_restored(&service, &provider).await;
    assert!(matches!(
        accounts.current.lock().await.credential_state,
        RemoteCredentialState::Missing
    ));
    for (fault, expected) in [
        (CredentialError::Locked, RemoteCredentialState::Locked),
        (
            CredentialError::Unavailable,
            RemoteCredentialState::Unavailable,
        ),
    ] {
        *backend.fault.lock().unwrap() = Some(fault);
        let accounts = RemoteAgentAccounts::with_store(Some(store.clone()));
        accounts.ensure_restored(&service, &provider).await;
        let current = accounts.current.lock().await;
        assert!(current.credential_state == expected);
        assert!(!current.can_sign_in(RemoteCredentialPersistence::SystemStore));
        assert!(current.account.is_none());
    }
    *backend.fault.lock().unwrap() = None;
    *backend.value.lock().unwrap() = Some("broken-json".into());
    let accounts = RemoteAgentAccounts::with_store(Some(store.clone()));
    accounts.ensure_restored(&service, &provider).await;
    assert!(matches!(
        accounts.current.lock().await.credential_state,
        RemoteCredentialState::Invalid
    ));
    for pending in [false, true] {
        let mut account = fixture("https://remote.example/");
        if pending {
            account.refresh_pending = true;
        } else {
            account.credentials.refresh_token_expires_at = 1;
        }
        persist_account(&account, Some(&store)).await.unwrap();
        let accounts = RemoteAgentAccounts::with_store(Some(store.clone()));
        accounts.ensure_restored(&service, &provider).await;
        assert!(matches!(
            accounts.current.lock().await.credential_state,
            RemoteCredentialState::ReauthenticationRequired
        ));
    }
}

#[tokio::test]
async fn clear_removes_invalid_saved_sign_in_even_when_no_account_restored() {
    let (service, provider) = service();
    let backend = Arc::new(Memory::default());
    let store = CredentialStore::from_backend(backend.clone());
    *backend.value.lock().unwrap() = Some("broken-json".into());
    let accounts = RemoteAgentAccounts::with_store(Some(store));
    accounts.ensure_restored(&service, &provider).await;
    let mut current = accounts.current.lock().await;
    assert!(current.account.is_none());
    current
        .clear(accounts.store.as_ref(), &service, &provider)
        .await
        .unwrap();
    assert!(backend.value.lock().unwrap().is_none());
    assert!(current.can_sign_in(RemoteCredentialPersistence::SystemStore));
    current
        .clear(accounts.store.as_ref(), &service, &provider)
        .await
        .unwrap();
    assert_eq!(backend.removes.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn failed_durable_clear_keeps_the_account_and_old_record_active() {
    let (service, provider) = service();
    let backend = Arc::new(Memory::default());
    let store = CredentialStore::from_backend(backend.clone());
    let account = fixture("https://remote.example/");
    let identity = account.identity.public_key.clone();
    persist_account(&account, Some(&store)).await.unwrap();
    let mut current = AccountState {
        account: Some(account),
        credential_state: RemoteCredentialState::Ready,
    };
    *backend.fail_remove.lock().unwrap() = true;
    assert!(current
        .clear(Some(&store), &service, &provider)
        .await
        .is_err());
    assert_eq!(
        current.account.as_ref().unwrap().identity.public_key,
        identity
    );
    assert!(backend.value.lock().unwrap().is_some());
    assert!(matches!(
        current.credential_state,
        RemoteCredentialState::Locked
    ));
    *backend.fail_remove.lock().unwrap() = false;
    current
        .clear(Some(&store), &service, &provider)
        .await
        .unwrap();
    assert!(current.account.is_none());
    assert!(backend.value.lock().unwrap().is_none());
}

#[tokio::test]
async fn retry_loaded_account_rechecks_storage_without_refresh_or_agent_replay() {
    for pending in [false, true] {
        let backend = Arc::new(Memory::default());
        let store = CredentialStore::from_backend(backend.clone());
        let mut account = fixture("https://remote.example/");
        account.refresh_pending = pending;
        let identity = account.identity.public_key.clone();
        let accounts = RemoteAgentAccounts::with_store(Some(store));
        {
            let mut current = accounts.current.lock().await;
            current.account = Some(account);
            current.credential_state = RemoteCredentialState::Locked;
        }
        let (service, provider) = service();
        *backend.fault.lock().unwrap() = Some(CredentialError::Locked);
        accounts.ensure_restored(&service, &provider).await;
        assert!(matches!(
            accounts.current.lock().await.credential_state,
            RemoteCredentialState::Locked
        ));
        *backend.fault.lock().unwrap() = None;
        accounts.ensure_restored(&service, &provider).await;
        let current = accounts.current.lock().await;
        assert_eq!(
            current.account.as_ref().unwrap().identity.public_key,
            identity
        );
        assert!(
            current.credential_state
                == if pending {
                    RemoteCredentialState::ReauthenticationRequired
                } else {
                    RemoteCredentialState::Ready
                }
        );
        assert_eq!(stored(&backend)["refreshPending"], json!(pending));
        assert_eq!(backend.writes.load(Ordering::SeqCst), 2);
    }
}

#[tokio::test]
async fn pin_write_failure_does_not_activate_or_replace_live_pins() {
    let backend = Arc::new(Memory::default());
    let store = CredentialStore::from_backend(backend.clone());
    let account = fixture("https://remote.example/");
    let connection = account.view.info.connection.clone();
    persist_account(&account, Some(&store)).await.unwrap();
    let old = account.pins.clone();
    let key = encode_base64(&[4; 32]);
    let mut current = AccountState {
        account: Some(account),
        credential_state: RemoteCredentialState::Ready,
    };
    *backend.fail_write_at.lock().unwrap() = Some(2);
    assert!(current
        .pin(&connection, "new-host".into(), key.clone(), Some(&store))
        .await
        .is_err());
    assert_eq!(current.account.as_ref().unwrap().pins, old);
    // An uncertain native write quarantines facade reads until an explicit write/remove succeeds.
    let error = match load_account(&store, now_ms().unwrap()).await {
        Err(error) => error,
        Ok(_) => panic!("Uncertain credential writes must quarantine reads"),
    };
    assert_eq!(error.code(), "remote_agent_credentials_unavailable");
    assert!(stored(&backend)["pins"].get("new-host").is_none());
    *backend.fail_write_at.lock().unwrap() = None;
    current
        .pin(&connection, "new-host".into(), key.clone(), Some(&store))
        .await
        .unwrap();
    assert_eq!(
        load_account(&store, now_ms().unwrap())
            .await
            .unwrap()
            .unwrap()
            .pins
            .get("new-host"),
        Some(&key)
    );
}

async fn http_once(
    body: Option<Value>,
    backend: Arc<Memory>,
    expected_path: &'static str,
    require_pending: bool,
) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}/", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let (mut socket, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
            .await
            .unwrap()
            .unwrap();
        let mut request = vec![0; 8192];
        let length = socket.read(&mut request).await.unwrap();
        assert!(String::from_utf8_lossy(&request[..length]).starts_with(expected_path));
        if require_pending {
            assert_eq!(stored(&backend)["refreshPending"], json!(true));
        }
        if let Some(body) = body {
            let body = body.to_string();
            let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
            socket.write_all(response.as_bytes()).await.unwrap();
        }
    });
    (origin, task)
}
fn rotated_tokens() -> Value {
    let now = now_ms().unwrap();
    json!({"accessToken":"rotated-access-fixture", "accessTokenExpiresAt":now+3_600_000, "refreshToken":"rotated-refresh-fixture", "refreshTokenExpiresAt":now+86_400_000})
}
#[tokio::test]
async fn refresh_uncertainty_is_durable_before_http_and_never_replayed() {
    let backend = Arc::new(Memory::default());
    let store = CredentialStore::from_backend(backend.clone());
    let (origin, server) =
        http_once(None, backend.clone(), "POST /api/v1/auth/refresh ", true).await;
    let mut account = fixture(&origin);
    account.credentials.access_token_expires_at = 1;
    persist_account(&account, Some(&store)).await.unwrap();
    let provider = DshRemoteProvider::default();
    assert!(refresh_account(&mut account, &provider, Some(&store))
        .await
        .is_err());
    server.await.unwrap();
    assert!(account.refresh_pending);
    assert_eq!(stored(&backend)["refreshPending"], json!(true));
    let writes = backend.writes.load(Ordering::SeqCst);
    assert!(refresh_account(&mut account, &provider, Some(&store))
        .await
        .is_err());
    assert_eq!(backend.writes.load(Ordering::SeqCst), writes);
    assert!(load_account(&store, now_ms().unwrap()).await.is_err());
}
#[tokio::test]
async fn serialized_refresh_saves_rotated_tokens_before_clearing_uncertainty() {
    let backend = Arc::new(Memory::default());
    let store = CredentialStore::from_backend(backend.clone());
    let (origin, server) = http_once(
        Some(rotated_tokens()),
        backend.clone(),
        "POST /api/v1/auth/refresh ",
        true,
    )
    .await;
    let mut account = fixture(&origin);
    account.credentials.access_token_expires_at = 1;
    let connection = account.view.info.connection.clone();
    let (_, provider) = service();
    provider
        .configure(connection.connection_id.0.clone(), configuration(&account))
        .await
        .unwrap();
    persist_account(&account, Some(&store)).await.unwrap();
    let accounts = RemoteAgentAccounts::with_store(Some(store.clone()));
    accounts.current.lock().await.account = Some(account);
    let (first, second) = tokio::join!(
        super::super::preflight(&connection, &accounts, &provider),
        super::super::preflight(&connection, &accounts, &provider)
    );
    first.unwrap();
    second.unwrap();
    server.await.unwrap();
    let saved = stored(&backend);
    assert_eq!(saved["refreshPending"], json!(false));
    assert_eq!(
        saved["credentials"]["refreshToken"],
        json!("rotated-refresh-fixture")
    );
    assert_eq!(backend.writes.load(Ordering::SeqCst), 4);
    assert!(
        !accounts
            .current
            .lock()
            .await
            .account
            .as_ref()
            .unwrap()
            .refresh_pending
    );
    assert!(load_account(&store, now_ms().unwrap()).await.is_ok());
}
#[tokio::test]
async fn failed_final_refresh_write_retains_durable_uncertainty_with_rotated_tokens() {
    let backend = Arc::new(Memory::default());
    let store = CredentialStore::from_backend(backend.clone());
    let (origin, server) = http_once(
        Some(rotated_tokens()),
        backend.clone(),
        "POST /api/v1/auth/refresh ",
        true,
    )
    .await;
    let mut account = fixture(&origin);
    account.credentials.access_token_expires_at = 1;
    let (_, provider) = service();
    provider
        .configure(
            account.view.info.connection.connection_id.0.clone(),
            configuration(&account),
        )
        .await
        .unwrap();
    persist_account(&account, Some(&store)).await.unwrap();
    *backend.fail_write_at.lock().unwrap() = Some(4);
    assert!(refresh_account(&mut account, &provider, Some(&store))
        .await
        .is_err());
    server.await.unwrap();
    assert!(account.refresh_pending);
    assert_eq!(stored(&backend)["refreshPending"], json!(true));
    assert_eq!(
        stored(&backend)["credentials"]["refreshToken"],
        json!("rotated-refresh-fixture")
    );
    assert!(load_account(&store, now_ms().unwrap()).await.is_err());
}
#[tokio::test]
async fn valid_restart_uses_discovery_only_and_does_not_replay_agent_operations() {
    let backend = Arc::new(Memory::default());
    let store = CredentialStore::from_backend(backend.clone());
    let (origin, server) = http_once(
        Some(json!({"items":[],"nextCursor":null})),
        backend.clone(),
        "GET /api/v1/devices?",
        false,
    )
    .await;
    let account = fixture(&origin);
    let connection = account.view.info.connection.clone();
    let public = account.identity.public_key.clone();
    persist_account(&account, Some(&store)).await.unwrap();
    let (service, provider) = service();
    let accounts = RemoteAgentAccounts::with_store(Some(store));
    accounts.ensure_restored(&service, &provider).await;
    server.await.unwrap();
    let current = accounts.current.lock().await;
    assert!(matches!(
        current.credential_state,
        RemoteCredentialState::Ready
    ));
    assert_eq!(
        current.account.as_ref().unwrap().identity.public_key,
        public
    );
    assert_eq!(
        current.account.as_ref().unwrap().view.info.connection,
        connection
    );
    assert!(service
        .local_outcome(
            &connection,
            &OperationId("operation-from-previous-launch".into())
        )
        .await
        .unwrap()
        .is_none());
}
#[tokio::test]
async fn desktop_without_store_remains_launch_only() {
    let accounts = RemoteAgentAccounts::with_store(None);
    assert!(accounts.persistence() == RemoteCredentialPersistence::LaunchOnly);
    assert!(accounts
        .current
        .lock()
        .await
        .can_sign_in(accounts.persistence()));
}
