//! Host-owned sync secrets. Public configuration holds an opaque generation only.
use super::{
    config::{self, SyncBackendConfig, SyncBackendKind, VaultSyncConfig},
    engine,
};
use crate::{
    core::error::AppError,
    integration::credentials::{CredentialError, CredentialStore},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{path::Path, sync::Arc};
use tauri::{AppHandle, Manager};

static MUTATIONS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

trait ConfigStorage: Send + Sync {
    fn get(&self, vault: &str) -> Result<Option<VaultSyncConfig>, AppError>;
    fn set(&self, vault: &str, config: VaultSyncConfig) -> Result<(), AppError>;
    fn remove(&self, vault: &str) -> Result<(), AppError>;
}
struct CoreConfig;
impl ConfigStorage for CoreConfig {
    fn get(&self, vault: &str) -> Result<Option<VaultSyncConfig>, AppError> {
        config::try_get(vault)
    }
    fn set(&self, vault: &str, value: VaultSyncConfig) -> Result<(), AppError> {
        config::set(vault, value)
    }
    fn remove(&self, vault: &str) -> Result<(), AppError> {
        config::remove(vault)
    }
}
#[derive(Default)]
pub struct SyncCredentialService(
    Arc<std::sync::Mutex<std::collections::HashMap<String, SecretVersions>>>,
);
struct Repository {
    store: Option<CredentialStore>,
    metadata: Arc<dyn ConfigStorage>,
    recovery: Arc<std::sync::Mutex<std::collections::HashMap<String, SecretVersions>>>,
}

// Neither Debug nor specta::Type. These values must never become IPC responses.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SecretVersions {
    version: u32,
    vault_digest: String,
    generations: Vec<SecretGeneration>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SecretGeneration {
    id: String,
    target_digest: String,
    secret_key: String,
    webdav_password: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session_token: Option<String>,
}

fn digest(value: &[u8]) -> String {
    hex::encode(Sha256::digest(value))
}
fn vault_digest(vault: &str) -> String {
    digest(vault.as_bytes())
}
fn record_key(vault: &str) -> String {
    format!("sync.credentials.v1.{}", vault_digest(vault))
}
fn invalid() -> AppError {
    AppError::domain(
        "sync_credentials_invalid",
        "Saved sync credentials are invalid or do not belong to this connection",
    )
}
fn missing() -> AppError {
    AppError::domain(
        "sync_credentials_missing",
        "Saved sync credentials are missing; enter the connection password again",
    )
}
fn storage_error(error: CredentialError) -> AppError {
    match error {
        CredentialError::Locked => AppError::domain(
            "sync_credentials_locked",
            "Unlock the system credential store and try again",
        ),
        CredentialError::Unavailable => AppError::domain(
            "sync_credentials_unavailable",
            "The system credential store is unavailable",
        ),
        _ => invalid(),
    }
}
fn supported(config: &SyncBackendConfig) -> Result<(), AppError> {
    if !matches!(
        config.backend,
        SyncBackendKind::S3 | SyncBackendKind::Webdav
    ) {
        return Err(AppError::domain(
            "sync_backend_unavailable_mobile",
            "Tablet sync supports S3 and WebDAV connections",
        ));
    }
    Ok(())
}
fn target_digest(config: &SyncBackendConfig) -> Result<String, AppError> {
    let normalized = config.clone().normalized();
    // Serialize structured fields: delimiter-based identities can collide when
    // a bucket, prefix or username itself contains the delimiter.
    let target = match normalized.backend {
        SyncBackendKind::S3 => serde_json::json!([
            "s3",
            normalized.endpoint,
            normalized.region,
            normalized.bucket,
            normalized.prefix,
            normalized.access_key,
            normalized.force_path_style
        ]),
        SyncBackendKind::Webdav => {
            serde_json::json!(["webdav", normalized.webdav_url, normalized.webdav_username])
        }
        _ => return Err(invalid()),
    };
    Ok(digest(&serde_json::to_vec(&target)?))
}
fn masked(value: &str) -> bool {
    !value.trim().is_empty() && value.trim().chars().all(|c| c == '*')
}
fn public_config(config: &SyncBackendConfig) -> SyncBackendConfig {
    let mut public = config.clone();
    clear_unused_protected_fields(&mut public);
    public.secret_key.clear();
    public.webdav_password.clear();
    public.session_token = None;
    public.agentero_token.clear();
    public
}
fn targets(binding: &VaultSyncConfig) -> impl Iterator<Item = &SyncBackendConfig> {
    std::iter::once(&binding.primary).chain(&binding.mirrors)
}
fn targets_mut(binding: &mut VaultSyncConfig) -> impl Iterator<Item = &mut SyncBackendConfig> {
    std::iter::once(&mut binding.primary).chain(&mut binding.mirrors)
}
fn protected_target(config: &SyncBackendConfig, mirror: bool) -> Result<String, AppError> {
    let target = target_digest(config)?;
    // Keep existing primary records readable, and keep mirror references from
    // being promoted into a primary binding by restored or modified metadata.
    Ok(if mirror {
        digest(format!("mirror:{target}").as_bytes())
    } else {
        target
    })
}
fn public_binding(binding: &VaultSyncConfig) -> VaultSyncConfig {
    VaultSyncConfig {
        primary: public_config(&binding.primary),
        mirrors: binding.mirrors.iter().map(public_config).collect(),
    }
}
impl SecretVersions {
    fn empty(vault: &str) -> Self {
        Self {
            version: 1,
            vault_digest: vault_digest(vault),
            generations: Vec::new(),
        }
    }
    fn validate(&self, vault: &str) -> Result<(), AppError> {
        if self.version != 1 || self.vault_digest != vault_digest(vault) {
            return Err(invalid());
        }
        let mut ids = std::collections::HashSet::new();
        for item in &self.generations {
            if !ids.insert(&item.id)
                || uuid::Uuid::parse_str(&item.id).is_err()
                || item.target_digest.len() != 64
                || !item.target_digest.bytes().all(|c| c.is_ascii_hexdigit())
            {
                return Err(invalid());
            }
        }
        Ok(())
    }
    fn resolve(
        &self,
        config: &SyncBackendConfig,
        mirror: bool,
    ) -> Result<SyncBackendConfig, AppError> {
        let reference = config.credential_ref.as_ref().ok_or_else(missing)?;
        let generation = self
            .generations
            .iter()
            .find(|item| &item.id == reference)
            .ok_or_else(missing)?;
        if generation.target_digest != protected_target(config, mirror)? {
            return Err(invalid());
        }
        let mut resolved = config.clone();
        resolved.secret_key = generation.secret_key.clone();
        resolved.webdav_password = generation.webdav_password.clone();
        if config.session_token.is_some() && generation.session_token.is_none() {
            // Older builds kept this token inline. A restored metadata file
            // cannot supply a trusted token for a protected credential.
            return Err(missing());
        }
        resolved.session_token = generation.session_token.clone();
        resolved.validate()?;
        Ok(resolved)
    }
}
impl Repository {
    fn app(app: &AppHandle) -> Self {
        Self {
            store: app.state::<Option<CredentialStore>>().inner().clone(),
            metadata: Arc::new(CoreConfig),
            recovery: app.state::<SyncCredentialService>().inner().0.clone(),
        }
    }
    async fn versions(&self, vault: &str) -> Result<SecretVersions, AppError> {
        let store = self
            .store
            .as_ref()
            .ok_or_else(|| storage_error(CredentialError::Unavailable))?;
        let pending = self
            .recovery
            .lock()
            .map_err(|_| invalid())?
            .get(vault)
            .cloned();
        if let Some(pending) = pending {
            // Retry only the exact known native write. Canonical metadata still
            // selects the old generation; this never activates a failed edit.
            store
                .write(&record_key(vault), &pending)
                .await
                .map_err(storage_error)?;
            self.recovery.lock().map_err(|_| invalid())?.remove(vault);
        }
        let versions = store
            .read::<SecretVersions>(&record_key(vault))
            .await
            .map_err(storage_error)?
            .unwrap_or_else(|| SecretVersions::empty(vault));
        versions.validate(vault)?;
        Ok(versions)
    }
    fn protected(&self) -> bool {
        cfg!(mobile) || self.store.is_some()
    }

    fn resolve_target(
        &self,
        config: &SyncBackendConfig,
        mirror: bool,
        versions: &Result<Option<SecretVersions>, AppError>,
    ) -> Result<SyncBackendConfig, AppError> {
        if self.protected() {
            supported(config)?;
            if let Err(error) = versions {
                return Err(AppError::domain(error.code(), error.to_string()));
            }
        }
        if config.credential_ref.is_some() {
            return versions
                .as_ref()
                .ok()
                .and_then(Option::as_ref)
                .ok_or_else(|| storage_error(CredentialError::Unavailable))?
                .resolve(config, mirror);
        }
        if self.protected() {
            config.validate()?;
        }
        Ok(config.clone())
    }

    async fn read_versions(&self, vault: &str) -> Result<Option<SecretVersions>, AppError> {
        if self.protected() {
            self.versions(vault).await.map(Some)
        } else {
            Ok(None)
        }
    }

    async fn load(&self, vault: &str) -> Result<Option<VaultSyncConfig>, AppError> {
        let Some(public) = self.metadata.get(vault)? else {
            return Ok(None);
        };
        let versions = self.read_versions(vault).await;
        let primary = self.resolve_target(&public.primary, false, &versions)?;
        let mirrors: Vec<_> = public
            .mirrors
            .iter()
            .map(|mirror| self.resolve_target(mirror, true, &versions))
            .collect();
        if self.protected()
            && targets(&public).any(|cfg| cfg.credential_ref.is_none())
            && mirrors.iter().all(Result::is_ok)
        {
            // Retire legacy inline secrets only after the native write succeeds.
            let binding = VaultSyncConfig {
                primary,
                mirrors: mirrors.into_iter().collect::<Result<_, _>>()?,
            };
            return self.save(vault, binding).await.map(Some);
        }
        if self.protected() && public.primary.credential_ref.is_none() {
            return Err(invalid());
        }
        Ok(Some(VaultSyncConfig {
            primary,
            mirrors: mirrors
                .into_iter()
                .zip(&public.mirrors)
                .map(|(resolved, metadata)| {
                    if self.protected() && metadata.credential_ref.is_none() {
                        return public_config(metadata);
                    }
                    resolved.unwrap_or_else(|_| public_config(metadata))
                })
                .collect(),
        }))
    }

    async fn save(
        &self,
        vault: &str,
        mut binding: VaultSyncConfig,
    ) -> Result<VaultSyncConfig, AppError> {
        if !self.protected() {
            for target in targets_mut(&mut binding) {
                target.credential_ref = None;
            }
            self.metadata.set(vault, binding.clone())?;
            return Ok(binding);
        }
        let store = self
            .store
            .as_ref()
            .ok_or_else(|| storage_error(CredentialError::Unavailable))?;
        for target in targets(&binding) {
            supported(target)?;
        }
        binding.validate()?;
        let prior = self.metadata.get(vault)?;
        let mut versions = self.versions(vault).await?;
        let previous: std::collections::HashSet<_> = prior
            .iter()
            .flat_map(targets)
            .filter_map(|cfg| cfg.credential_ref.as_ref())
            .collect();
        if !previous.is_empty() {
            // Confirm visible metadata durably before dropping any prior slot.
            // Keep legacy inline fields until the new protected write succeeds.
            self.metadata
                .set(vault, prior.clone().ok_or_else(invalid)?)?;
        }
        versions
            .generations
            .retain(|item| previous.contains(&item.id));
        for (index, target) in targets_mut(&mut binding).enumerate() {
            let id = uuid::Uuid::new_v4().to_string();
            versions.generations.push(SecretGeneration {
                id: id.clone(),
                target_digest: protected_target(target, index != 0)?,
                secret_key: target.secret_key.clone(),
                webdav_password: target.webdav_password.clone(),
                session_token: target.session_token.clone(),
            });
            target.credential_ref = Some(id);
        }
        if let Err(error) = store.write(&record_key(vault), &versions).await {
            if matches!(
                error,
                CredentialError::Locked | CredentialError::Unavailable
            ) {
                self.recovery
                    .lock()
                    .map_err(|_| invalid())?
                    .insert(vault.into(), versions);
            }
            return Err(storage_error(error));
        }
        self.metadata.set(vault, public_binding(&binding))?;
        Ok(binding)
    }
    async fn clear(&self, vault: &str) -> Result<(), AppError> {
        if let Some(store) = &self.store {
            store
                .remove(&record_key(vault))
                .await
                .map_err(storage_error)?;
        } else if self.metadata.get(vault)?.is_some_and(|binding| {
            self.protected() || targets(&binding).any(|cfg| cfg.credential_ref.is_some())
        }) {
            return Err(storage_error(CredentialError::Unavailable));
        }
        self.recovery.lock().map_err(|_| invalid())?.remove(vault);
        self.metadata.remove(vault)
    }
}

pub async fn load(app: &AppHandle, vault: &str) -> Result<Option<VaultSyncConfig>, AppError> {
    let _guard = MUTATIONS.lock().await;
    Repository::app(app).load(vault).await
}

#[derive(Debug, Clone, Copy, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum SyncCredentialState {
    NotConfigured,
    Inline,
    Ready,
    Locked,
    Unavailable,
    Missing,
    Invalid,
}
pub struct CredentialStatus {
    pub primary: Option<TargetStatus>,
    pub mirrors: Vec<TargetStatus>,
}
pub struct TargetStatus {
    pub config: SyncBackendConfig,
    pub state: SyncCredentialState,
}

impl CredentialStatus {
    pub fn configured(binding: &VaultSyncConfig, protected: bool) -> Self {
        let state = if protected {
            SyncCredentialState::Ready
        } else {
            SyncCredentialState::Inline
        };
        let status = |config: &SyncBackendConfig| TargetStatus {
            config: masked_status_config(config),
            state,
        };
        Self {
            primary: Some(status(&binding.primary)),
            mirrors: binding.mirrors.iter().map(status).collect(),
        }
    }
}

pub(super) fn masked_status_config(config: &SyncBackendConfig) -> SyncBackendConfig {
    let mut public = config.masked();
    if config.credential_ref.is_some() {
        match config.backend {
            SyncBackendKind::S3 => public.secret_key = "********".into(),
            SyncBackendKind::Webdav => public.webdav_password = "********".into(),
            _ => {}
        }
    }
    public.credential_ref = None;
    for field in [
        &mut public.endpoint,
        &mut public.webdav_url,
        &mut public.agentero_api,
        &mut public.git_remote,
    ] {
        if let Ok(mut url) = url::Url::parse(field) {
            if !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
            {
                let _ = url.set_password(None);
                let _ = url.set_username("");
                url.set_query(None);
                url.set_fragment(None);
                *field = url.to_string();
            }
        }
    }
    public
}
pub async fn status(app: &AppHandle, vault: &str) -> Result<CredentialStatus, AppError> {
    let _guard = MUTATIONS.lock().await;
    let repository = Repository::app(app);
    let Some(public) = repository.metadata.get(vault)? else {
        return Ok(CredentialStatus {
            primary: None,
            mirrors: Vec::new(),
        });
    };
    let mut migration_error = None;
    if repository.protected() && targets(&public).any(|cfg| cfg.credential_ref.is_none()) {
        // Keep migrating the original single-backend file when settings opens.
        match repository.load(vault).await {
            Ok(Some(binding)) if targets(&binding).all(|cfg| cfg.credential_ref.is_some()) => {
                return Ok(CredentialStatus::configured(&binding, true));
            }
            Err(error) => migration_error = Some(credential_state(&error)),
            _ => {}
        }
    }
    let versions = repository.read_versions(vault).await;
    let status = |config: &SyncBackendConfig, mirror| match repository
        .resolve_target(config, mirror, &versions)
    {
        Ok(resolved) => TargetStatus {
            config: masked_status_config(&resolved),
            state: if config.credential_ref.is_some() {
                SyncCredentialState::Ready
            } else if repository.protected() {
                migration_error.unwrap_or(SyncCredentialState::Invalid)
            } else {
                SyncCredentialState::Inline
            },
        },
        Err(error) => TargetStatus {
            config: masked_status_config(config),
            state: credential_state(&error),
        },
    };
    Ok(CredentialStatus {
        primary: Some(status(&public.primary, false)),
        mirrors: public.mirrors.iter().map(|cfg| status(cfg, true)).collect(),
    })
}

fn credential_state(error: &AppError) -> SyncCredentialState {
    match error.code() {
        "sync_credentials_locked" => SyncCredentialState::Locked,
        "sync_credentials_unavailable" => SyncCredentialState::Unavailable,
        "sync_credentials_missing" => SyncCredentialState::Missing,
        _ => SyncCredentialState::Invalid,
    }
}

fn clear_unused_protected_fields(target: &mut SyncBackendConfig) {
    target.git_remote.clear();
    target.git_ssl_ca_info.clear();
    target.agentero_api.clear();
    target.agentero_token.clear();
    if target.backend == SyncBackendKind::S3 {
        target.webdav_url.clear();
        target.webdav_username.clear();
        target.webdav_password.clear();
    } else {
        target.endpoint.clear();
        target.access_key.clear();
        target.secret_key.clear();
        target.session_token = None;
    }
}

fn uses_mask(config: &SyncBackendConfig) -> bool {
    match config.backend {
        SyncBackendKind::S3 => {
            masked(&config.secret_key) || config.session_token.as_deref().is_some_and(masked)
        }
        SyncBackendKind::Webdav => masked(&config.webdav_password),
        _ => false,
    }
}

fn merge_protected_mask(
    repository: &Repository,
    candidate: &mut SyncBackendConfig,
    previous: Option<&SyncBackendConfig>,
    mirror: bool,
    versions: &Result<Option<SecretVersions>, AppError>,
) -> Result<(), AppError> {
    supported(candidate)?;
    if !uses_mask(candidate) {
        return Ok(());
    }
    let prior = previous.ok_or_else(missing)?;
    if target_digest(prior)? != target_digest(candidate)? {
        return Err(AppError::domain(
            "sync_credentials_target_changed",
            "Enter the password again when changing the sync destination or account",
        ));
    }
    let resolved = repository.resolve_target(prior, mirror, versions)?;
    candidate.merge_mask(Some(&resolved));
    Ok(())
}

pub struct Configured {
    pub binding: VaultSyncConfig,
    pub protected: bool,
}
/// Caller owns the vault's sync lease and cross-process SyncLock.
pub async fn configure(
    app: &AppHandle,
    vault: &str,
    dir: &Path,
    mut binding: VaultSyncConfig,
) -> Result<Configured, AppError> {
    let _guard = MUTATIONS.lock().await;
    let repository = Repository::app(app);
    let previous = repository.metadata.get(vault)?;
    let protected = repository.protected();
    if protected {
        let versions = Ok(Some(repository.versions(vault).await?));
        merge_protected_mask(
            &repository,
            &mut binding.primary,
            previous.as_ref().map(|binding| &binding.primary),
            false,
            &versions,
        )?;
        for mirror in &mut binding.mirrors {
            supported(mirror)?;
            let destination = target_digest(mirror)?;
            let prior = previous.as_ref().and_then(|binding| {
                binding
                    .mirrors
                    .iter()
                    .find(|prior| target_digest(prior).is_ok_and(|target| target == destination))
            });
            merge_protected_mask(&repository, mirror, prior, true, &versions)?;
        }
    } else {
        if previous
            .as_ref()
            .is_some_and(|binding| targets(binding).any(|cfg| cfg.credential_ref.is_some()))
        {
            return Err(storage_error(CredentialError::Unavailable));
        }
        binding.merge_mask(previous.as_ref());
    }
    for target in targets_mut(&mut binding) {
        target.credential_ref = None;
        if protected {
            clear_unused_protected_fields(target);
        }
    }
    binding.validate()?;
    if binding.duplicate_remote().is_some() {
        return Err(AppError::message(
            "the same remote is configured twice; each backend needs its own store",
        ));
    }
    let probe_error = |error: AppError| {
        if protected {
            AppError::domain(error.code(), "Could not verify the sync connection")
        } else {
            error
        }
    };
    binding.primary.conditional_writes = engine::test_connection(dir, &binding.primary)
        .await
        .map_err(probe_error)?;
    for mirror in &mut binding.mirrors {
        mirror.conditional_writes =
            engine::test_mirror_connection(dir, mirror)
                .await
                .map_err(|error| {
                    if error.code() == agentero_core::features::sync::SYNC_NO_CONDITIONAL_WRITES {
                        AppError::domain(
                            "sync_mirror_no_conditional_writes",
                            "This mirror store does not enforce conditional writes",
                        )
                    } else {
                        probe_error(error)
                    }
                })?;
    }
    if let Some(previous) = previous.as_ref() {
        // Bind any legacy base before publishing a new authoritative target.
        super::local::bind_configured_store(dir, &previous.primary)?;
    }
    let binding = repository.save(vault, binding).await?;
    Ok(Configured { binding, protected })
}

pub async fn clear(app: &AppHandle, vault: &str) -> Result<(), AppError> {
    let _guard = MUTATIONS.lock().await;
    Repository::app(app).clear(vault).await
}

#[cfg(test)]
#[path = "credentials_tests.rs"]
mod tests;
