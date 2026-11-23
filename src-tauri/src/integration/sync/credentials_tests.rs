use super::*;
use crate::integration::credentials::CredentialBackend;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering},
        Mutex,
    },
};

#[derive(Default)]
struct Metadata {
    values: Mutex<HashMap<String, VaultSyncConfig>>,
    fail_set: AtomicU8,
    fail_on_call: AtomicUsize,
    set_calls: AtomicUsize,
    fail_remove: AtomicBool,
}
impl ConfigStorage for Metadata {
    fn get(&self, key: &str) -> Result<Option<VaultSyncConfig>, AppError> {
        Ok(self.values.lock().unwrap().get(key).cloned())
    }
    fn set(&self, key: &str, cfg: VaultSyncConfig) -> Result<(), AppError> {
        let call = self.set_calls.fetch_add(1, Ordering::SeqCst) + 1;
        let selected = self.fail_on_call.load(Ordering::SeqCst);
        let failure = if selected == 0 || selected == call {
            self.fail_set.load(Ordering::SeqCst)
        } else {
            0
        };
        if failure != 1 {
            self.values.lock().unwrap().insert(key.into(), cfg);
        }
        if failure != 0 {
            Err(AppError::message("metadata write failed"))
        } else {
            Ok(())
        }
    }
    fn remove(&self, key: &str) -> Result<(), AppError> {
        if self.fail_remove.load(Ordering::SeqCst) {
            return Err(AppError::message("metadata removal failed"));
        }
        self.values.lock().unwrap().remove(key);
        Ok(())
    }
}
#[derive(Default)]
struct Backend {
    disk: Arc<Mutex<HashMap<String, String>>>,
    cache: Mutex<HashMap<String, String>>,
    locked: AtomicBool,
    fail_write: AtomicBool,
    fail_remove: AtomicBool,
}
impl Backend {
    fn restart(&self) -> Self {
        Self {
            disk: self.disk.clone(),
            cache: Mutex::new(self.disk.lock().unwrap().clone()),
            ..Self::default()
        }
    }
}
impl CredentialBackend for Backend {
    fn read(&self, key: &str) -> Result<Option<String>, CredentialError> {
        if self.locked.load(Ordering::SeqCst) {
            return Err(CredentialError::Locked);
        }
        Ok(self.cache.lock().unwrap().get(key).cloned())
    }
    fn write(&self, key: &str, value: &str) -> Result<(), CredentialError> {
        if self.locked.load(Ordering::SeqCst) {
            return Err(CredentialError::Locked);
        }
        self.cache.lock().unwrap().insert(key.into(), value.into());
        if self.fail_write.load(Ordering::SeqCst) {
            return Err(CredentialError::Unavailable);
        }
        *self.disk.lock().unwrap() = self.cache.lock().unwrap().clone();
        Ok(())
    }
    fn remove(&self, key: &str) -> Result<(), CredentialError> {
        if self.locked.load(Ordering::SeqCst) {
            return Err(CredentialError::Locked);
        }
        self.cache.lock().unwrap().remove(key);
        if self.fail_remove.load(Ordering::SeqCst) {
            return Err(CredentialError::Unavailable);
        }
        *self.disk.lock().unwrap() = self.cache.lock().unwrap().clone();
        Ok(())
    }
}
fn repository(backend: Arc<Backend>, metadata: Arc<Metadata>) -> Repository {
    Repository {
        store: Some(CredentialStore::from_backend(backend)),
        metadata,
        recovery: Default::default(),
    }
}
fn s3(secret: &str) -> SyncBackendConfig {
    SyncBackendConfig {
        endpoint: "https://s3.example.com".into(),
        bucket: "research".into(),
        access_key: "account-id".into(),
        secret_key: secret.into(),
        ..SyncBackendConfig::default()
    }
}

#[tokio::test]
async fn migration_protects_then_retires_plaintext_and_restores_after_restart() {
    let metadata = Arc::new(Metadata::default());
    let backend = Arc::new(Backend::default());
    metadata
        .set("vault", VaultSyncConfig::new(s3("unique-legacy-password")))
        .unwrap();
    let repo = repository(backend.clone(), metadata.clone());
    let loaded = repo.load("vault").await.unwrap().unwrap();
    assert_eq!(loaded.primary.secret_key, "unique-legacy-password");
    let public = metadata.get("vault").unwrap().unwrap();
    assert!(public.primary.credential_ref.is_some());
    assert!(public.primary.secret_key.is_empty());
    assert!(!serde_json::to_string(&public.primary)
        .unwrap()
        .contains("unique-legacy-password"));
    let rebooted = repository(Arc::new(backend.restart()), metadata);
    assert_eq!(
        rebooted
            .load("vault")
            .await
            .unwrap()
            .unwrap()
            .primary
            .secret_key,
        "unique-legacy-password"
    );
}

#[tokio::test]
async fn locked_migration_retains_legacy_and_never_claims_new_storage() {
    let metadata = Arc::new(Metadata::default());
    let backend = Arc::new(Backend::default());
    metadata
        .set("vault", VaultSyncConfig::new(s3("old-password")))
        .unwrap();
    backend.locked.store(true, Ordering::SeqCst);
    let repo = repository(backend.clone(), metadata.clone());
    assert_eq!(
        repo.load("vault").await.unwrap_err().code(),
        "sync_credentials_locked"
    );
    assert_eq!(
        metadata.get("vault").unwrap().unwrap().primary.secret_key,
        "old-password"
    );
    assert!(backend.disk.lock().unwrap().is_empty());
    backend.locked.store(false, Ordering::SeqCst);
    assert!(repo.load("vault").await.is_ok());
}

#[tokio::test]
async fn failed_native_cache_changing_write_retries_only_known_bytes_and_keeps_old_reference() {
    let metadata = Arc::new(Metadata::default());
    let backend = Arc::new(Backend::default());
    let repo = repository(backend.clone(), metadata.clone());
    let old = repo
        .save("vault", VaultSyncConfig::new(s3("old-password")))
        .await
        .unwrap();
    backend.fail_write.store(true, Ordering::SeqCst);
    assert!(repo
        .save("vault", VaultSyncConfig::new(s3("new-password")))
        .await
        .is_err());
    assert_eq!(
        metadata
            .get("vault")
            .unwrap()
            .unwrap()
            .primary
            .credential_ref,
        old.primary.credential_ref
    );
    assert!(repo.load("vault").await.is_err());
    backend.fail_write.store(false, Ordering::SeqCst);
    assert_eq!(
        repo.load("vault")
            .await
            .unwrap()
            .unwrap()
            .primary
            .secret_key,
        "old-password"
    );
    assert!(repo.recovery.lock().unwrap().is_empty());
}

#[tokio::test]
async fn interrupted_metadata_publication_keeps_both_possible_password_generations() {
    for failure in [1, 2] {
        let metadata = Arc::new(Metadata::default());
        let backend = Arc::new(Backend::default());
        let repo = repository(backend.clone(), metadata.clone());
        repo.save("vault", VaultSyncConfig::new(s3("old-password")))
            .await
            .unwrap();
        metadata.fail_on_call.store(
            metadata.set_calls.load(Ordering::SeqCst) + 2,
            Ordering::SeqCst,
        );
        metadata.fail_set.store(failure, Ordering::SeqCst);
        assert!(repo
            .save("vault", VaultSyncConfig::new(s3("new-password")))
            .await
            .is_err());
        metadata.fail_set.store(0, Ordering::SeqCst);
        let rebooted = repository(Arc::new(backend.restart()), metadata);
        assert_eq!(
            rebooted
                .load("vault")
                .await
                .unwrap()
                .unwrap()
                .primary
                .secret_key,
            if failure == 1 {
                "old-password"
            } else {
                "new-password"
            }
        );
    }
}

#[tokio::test]
async fn retry_cannot_prune_the_last_confirmed_generation_after_visible_unconfirmed_metadata() {
    let metadata = Arc::new(Metadata::default());
    let backend = Arc::new(Backend::default());
    let repo = repository(backend.clone(), metadata.clone());
    repo.save("vault", VaultSyncConfig::new(s3("durable-r")))
        .await
        .unwrap();
    let durable = metadata.get("vault").unwrap().unwrap();
    metadata.fail_on_call.store(
        metadata.set_calls.load(Ordering::SeqCst) + 2,
        Ordering::SeqCst,
    );
    metadata.fail_set.store(2, Ordering::SeqCst);
    assert!(repo
        .save("vault", VaultSyncConfig::new(s3("unconfirmed-n")))
        .await
        .is_err());
    assert_ne!(
        metadata
            .get("vault")
            .unwrap()
            .unwrap()
            .primary
            .credential_ref,
        durable.primary.credential_ref
    );
    // The next attempt cannot confirm N. It must stop before replacing R+N.
    metadata.fail_on_call.store(
        metadata.set_calls.load(Ordering::SeqCst) + 1,
        Ordering::SeqCst,
    );
    metadata.fail_set.store(1, Ordering::SeqCst);
    let protected = backend.disk.lock().unwrap().clone();
    assert!(repo
        .save("vault", VaultSyncConfig::new(s3("candidate-m")))
        .await
        .is_err());
    assert_eq!(*backend.disk.lock().unwrap(), protected);
    // A power loss rolls the visible but unconfirmed rename back to durable R.
    metadata
        .values
        .lock()
        .unwrap()
        .insert("vault".into(), durable);
    metadata.fail_set.store(0, Ordering::SeqCst);
    let rebooted = repository(Arc::new(backend.restart()), metadata);
    assert_eq!(
        rebooted
            .load("vault")
            .await
            .unwrap()
            .unwrap()
            .primary
            .secret_key,
        "durable-r"
    );
}

#[tokio::test]
async fn protected_passwords_are_bound_to_target_account_backend_and_vault() {
    let metadata = Arc::new(Metadata::default());
    let backend = Arc::new(Backend::default());
    let repo = repository(backend.clone(), metadata.clone());
    repo.save("vault", VaultSyncConfig::new(s3("protected-password")))
        .await
        .unwrap();
    let original = metadata.get("vault").unwrap().unwrap();
    for (index, mut changed) in [original.clone(), original.clone(), original.clone()]
        .into_iter()
        .enumerate()
    {
        match index {
            0 => changed.primary.endpoint = "https://attacker.example.com".into(),
            1 => changed.primary.bucket = "other-bucket".into(),
            _ => changed.primary.prefix = "other-prefix".into(),
        }
        metadata.set("vault", changed).unwrap();
        assert_eq!(
            repo.load("vault").await.unwrap_err().code(),
            "sync_credentials_invalid"
        );
    }
    let mut account = original.clone();
    account.primary.access_key = "other-account".into();
    metadata.set("vault", account).unwrap();
    assert!(repo.load("vault").await.is_err());
    let mut backend_swap = original.clone();
    backend_swap.primary.backend = SyncBackendKind::Webdav;
    backend_swap.primary.webdav_url = original.primary.endpoint.clone();
    backend_swap.primary.webdav_username = "other-account".into();
    metadata.set("vault", backend_swap).unwrap();
    assert!(repo.load("vault").await.is_err());
    metadata.set("other-vault", original).unwrap();
    assert_eq!(
        repo.load("other-vault").await.unwrap_err().code(),
        "sync_credentials_missing"
    );
    let raw = backend
        .cache
        .lock()
        .unwrap()
        .get(&record_key("vault"))
        .unwrap()
        .clone();
    backend.write(&record_key("other-vault"), &raw).unwrap();
    assert_eq!(
        repo.load("other-vault").await.unwrap_err().code(),
        "sync_credentials_invalid"
    );
}

#[tokio::test]
async fn missing_device_only_record_never_uses_inline_fallback_and_explicit_password_can_repair() {
    let metadata = Arc::new(Metadata::default());
    let backend = Arc::new(Backend::default());
    let repo = repository(backend.clone(), metadata.clone());
    repo.save("vault", VaultSyncConfig::new(s3("protected-password")))
        .await
        .unwrap();
    backend.cache.lock().unwrap().clear();
    backend.disk.lock().unwrap().clear();
    let mut public = metadata.get("vault").unwrap().unwrap();
    public.primary.secret_key = "untrusted-inline-fallback".into();
    metadata.set("vault", public.clone()).unwrap();
    assert_eq!(
        repo.load("vault").await.unwrap_err().code(),
        "sync_credentials_missing"
    );
    assert_eq!(masked_status_config(&public.primary).secret_key, "********");
    repo.save("vault", VaultSyncConfig::new(s3("reentered-password")))
        .await
        .unwrap();
    assert_eq!(
        repo.load("vault")
            .await
            .unwrap()
            .unwrap()
            .primary
            .secret_key,
        "reentered-password"
    );
}

#[tokio::test]
async fn failed_secure_delete_does_not_report_disconnection_or_remove_public_binding() {
    let metadata = Arc::new(Metadata::default());
    let backend = Arc::new(Backend::default());
    let repo = repository(backend.clone(), metadata.clone());
    repo.save("vault", VaultSyncConfig::new(s3("protected-password")))
        .await
        .unwrap();
    backend.fail_remove.store(true, Ordering::SeqCst);
    assert!(repo.clear("vault").await.is_err());
    assert!(metadata.get("vault").unwrap().is_some());
    backend.fail_remove.store(false, Ordering::SeqCst);
    repo.clear("vault").await.unwrap();
    assert!(metadata.get("vault").unwrap().is_none());
    assert!(backend.disk.lock().unwrap().is_empty());
}

#[tokio::test]
async fn failed_metadata_clear_stays_missing_and_explicit_retry_finishes_disconnect() {
    let metadata = Arc::new(Metadata::default());
    let backend = Arc::new(Backend::default());
    let repo = repository(backend.clone(), metadata.clone());
    repo.save("vault", VaultSyncConfig::new(s3("protected-password")))
        .await
        .unwrap();
    metadata.fail_remove.store(true, Ordering::SeqCst);
    assert!(repo.clear("vault").await.is_err());
    assert!(metadata.get("vault").unwrap().is_some());
    assert_eq!(
        repo.load("vault").await.unwrap_err().code(),
        "sync_credentials_missing"
    );
    metadata.fail_remove.store(false, Ordering::SeqCst);
    repo.clear("vault").await.unwrap();
    assert!(metadata.get("vault").unwrap().is_none());
    assert!(backend.disk.lock().unwrap().is_empty());
}

#[tokio::test]
async fn protected_mirrors_round_trip_after_reordering_and_reject_retargeted_secrets() {
    let metadata = Arc::new(Metadata::default());
    let backend = Arc::new(Backend::default());
    let repo = repository(backend.clone(), metadata.clone());
    let binding = VaultSyncConfig {
        primary: s3("primary-password"),
        mirrors: vec![
            SyncBackendConfig {
                bucket: "mirror".into(),
                session_token: Some("temporary-credential".into()),
                ..s3("mirror-password")
            },
            SyncBackendConfig {
                backend: SyncBackendKind::Webdav,
                webdav_url: "https://dav.example.com/vault".into(),
                webdav_username: "mirror-username".into(),
                webdav_password: "webdav-password".into(),
                ..SyncBackendConfig::default()
            },
        ],
    };
    repo.save("vault", binding).await.unwrap();
    let mut public = metadata.get("vault").unwrap().unwrap();
    assert!(public.mirrors[0].secret_key.is_empty());
    assert!(public.mirrors[0].session_token.is_none());
    assert!(public.mirrors[1].webdav_password.is_empty());
    public.mirrors.reverse();
    metadata.set("vault", public.clone()).unwrap();
    let rebooted = repository(Arc::new(backend.restart()), metadata.clone());
    let loaded = rebooted.load("vault").await.unwrap().unwrap();
    assert_eq!(loaded.mirrors[0].webdav_password, "webdav-password");
    assert_eq!(loaded.mirrors[1].secret_key, "mirror-password");
    assert_eq!(
        loaded.mirrors[1].session_token.as_deref(),
        Some("temporary-credential")
    );
    let status = serde_json::to_string(&masked_status_config(&loaded.mirrors[1])).unwrap();
    assert!(!status.contains("mirror-password"));
    assert!(!status.contains("temporary-credential"));
    assert!(!status.contains(public.mirrors[1].credential_ref.as_deref().unwrap()));

    public.mirrors[0].webdav_url = "https://other.example.com/vault".into();
    metadata.set("vault", public.clone()).unwrap();
    let loaded = rebooted.load("vault").await.unwrap().unwrap();
    assert_eq!(loaded.primary.secret_key, "primary-password");
    assert!(loaded.mirrors[0].validate().is_err());
    assert!(loaded.mirrors[0].webdav_password.is_empty());
    assert_eq!(loaded.mirrors[1].secret_key, "mirror-password");

    std::mem::swap(&mut public.primary, &mut public.mirrors[1]);
    metadata.set("vault", public).unwrap();
    assert_eq!(
        rebooted.load("vault").await.unwrap_err().code(),
        "sync_credentials_invalid"
    );
}

#[test]
fn target_encoding_is_unambiguous_and_status_never_echoes_url_passwords() {
    let mut a = s3("password");
    a.bucket = "a|b".into();
    a.prefix = "c".into();
    let mut b = a.clone();
    b.bucket = "a".into();
    b.prefix = "b|c".into();
    assert_ne!(a.remote_identity(), b.remote_identity());
    assert_ne!(target_digest(&a).unwrap(), target_digest(&b).unwrap());
    a.endpoint = "https://user:embedded-password@example.com".into();
    assert!(!serde_json::to_string(&masked_status_config(&a))
        .unwrap()
        .contains("embedded-password"));
}
