use crate::core::error::AppError;
use crate::core::fs::{atomic_write_with, with_file_mutation, AtomicOpts};
use crate::integration::credentials::{CredentialError, CredentialStore};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, Utc};
use crypto_box::aead::OsRng;
use crypto_box::{PublicKey, SecretKey};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const IDENTITY_FILE: &str = "identity.json";
const DEVICES_FILE: &str = "devices.json";
const CLIENT_IDENTITY_FILE: &str = "client-identity.json";
const CLIENT_PROFILE_FILE: &str = "client-profile.json";
#[cfg(target_os = "ios")]
const IOS_KEYCHAIN_SERVICE: &str = "com.poco-ai.agentero.bridge";
#[cfg(target_os = "ios")]
const IOS_KEYCHAIN_ACCOUNT: &str = "client-identity-v2";
// `security-framework-sys` does not export this status; the Security framework
// value is stable (`errSecInteractionNotAllowed` in Security/SecBase.h).
#[cfg(target_os = "ios")]
const ERR_SEC_INTERACTION_NOT_ALLOWED: i32 = -25308;

fn client_config_dir(app: &tauri::AppHandle) -> Result<PathBuf, AppError> {
    #[cfg(target_os = "android")]
    {
        use tauri::Manager;
        app.path()
            .app_config_dir()
            .map(|dir| dir.join("bridge"))
            .map_err(|error| {
                AppError::message(format!(
                    "Could not resolve Bridge config directory: {error}"
                ))
            })
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Ok(crate::core::paths::bridge_config_dir())
    }
}

/// Long-lived desktop identity. The secret key stays in the local config dir
/// and is never added to a QR offer or sent to the Relay.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BridgeIdentity {
    pub v: u8,
    pub server_id: String,
    pub public_key_b64: String,
    pub secret_key_b64: String,
    pub created_at: DateTime<Utc>,
}

impl BridgeIdentity {
    pub fn create() -> Self {
        let secret = SecretKey::generate(&mut OsRng);
        let public = secret.public_key();
        let server_id = format!(
            "agt_{}",
            URL_SAFE_NO_PAD.encode(uuid::Uuid::new_v4().as_bytes())
        );
        Self {
            v: 1,
            server_id,
            public_key_b64: URL_SAFE_NO_PAD.encode(public.as_bytes()),
            secret_key_b64: URL_SAFE_NO_PAD.encode(secret.to_bytes()),
            created_at: Utc::now(),
        }
    }

    pub fn secret_key(&self) -> Result<SecretKey, AppError> {
        let bytes = decode_32(&self.secret_key_b64, "Bridge secret key")?;
        Ok(SecretKey::from(bytes))
    }

    pub fn public_key(&self) -> Result<PublicKey, AppError> {
        let bytes = decode_32(&self.public_key_b64, "Bridge public key")?;
        Ok(PublicKey::from(bytes))
    }
}

/// A client device that was manually approved by the desktop user.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct BridgeDevice {
    pub device_id: String,
    pub name: String,
    pub public_key_b64: String,
    pub paired_at: DateTime<Utc>,
    pub last_seen_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub revoked: bool,
}

/// Long-lived Ed25519 identity for one mobile device. Its public half is sent
/// only in an encrypted `pair_request`; the secret half signs each future
/// connection challenge and is stored in the iOS Keychain on mobile.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BridgeClientIdentity {
    pub v: u8,
    pub device_id: String,
    pub public_key_b64: String,
    pub secret_key_b64: String,
    pub created_at: DateTime<Utc>,
}

/// The non-secret connection profile for the most recently paired desktop.
/// The device signing key remains in `BridgeClientIdentity`; this record only
/// tells iOS which Relay endpoint and server should be resumed at launch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BridgeClientProfile {
    pub offer_url: String,
    pub device_name: String,
    pub paired: bool,
}

impl BridgeClientIdentity {
    pub fn create() -> Self {
        let secret =
            SigningKey::from_bytes(&crypto_box::SecretKey::generate(&mut OsRng).to_bytes());
        let public = secret.verifying_key();
        Self {
            v: 2,
            device_id: format!(
                "ios_{}",
                URL_SAFE_NO_PAD.encode(uuid::Uuid::new_v4().as_bytes())
            ),
            public_key_b64: URL_SAFE_NO_PAD.encode(public.as_bytes()),
            secret_key_b64: URL_SAFE_NO_PAD.encode(secret.to_bytes()),
            created_at: Utc::now(),
        }
    }

    pub fn signing_key(&self) -> Result<SigningKey, AppError> {
        Ok(SigningKey::from_bytes(&decode_32(
            &self.secret_key_b64,
            "Bridge client signing key",
        )?))
    }

    pub fn verifying_key(&self) -> Result<VerifyingKey, AppError> {
        let bytes = decode_32(&self.public_key_b64, "Bridge client public key")?;
        VerifyingKey::from_bytes(&bytes)
            .map_err(|_| AppError::message("Bridge client public key is invalid"))
    }

    pub fn sign_challenge(&self, nonce: &[u8]) -> Result<String, AppError> {
        Ok(URL_SAFE_NO_PAD.encode(self.signing_key()?.sign(nonce).to_bytes()))
    }
}

pub fn verify_device_challenge(
    public_key_b64: &str,
    nonce: &[u8],
    signature_b64: &str,
) -> Result<(), AppError> {
    let public = decode_32(public_key_b64, "Bridge device public key")?;
    let public = VerifyingKey::from_bytes(&public)
        .map_err(|_| AppError::message("Bridge device public key is invalid"))?;
    let signature: [u8; 64] = URL_SAFE_NO_PAD
        .decode(signature_b64)
        .map_err(|_| AppError::message("Bridge device signature is not valid base64url"))?
        .try_into()
        .map_err(|_| AppError::message("Bridge device signature must contain 64 bytes"))?;
    public
        .verify(nonce, &Signature::from_bytes(&signature))
        .map_err(|_| AppError::message("Bridge device signature did not verify"))
}

pub fn validate_device_public_key(value: &str) -> Result<(), AppError> {
    let public = decode_32(value, "Bridge device public key")?;
    VerifyingKey::from_bytes(&public)
        .map(|_| ())
        .map_err(|_| AppError::message("Bridge device public key is invalid"))
}

#[derive(Clone)]
pub struct BridgeIdentityStore {
    dir: PathBuf,
}

impl BridgeIdentityStore {
    pub fn at_default_path() -> Self {
        Self {
            dir: crate::core::paths::bridge_config_dir(),
        }
    }

    #[cfg(test)]
    pub fn at_path(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn load_or_create(&self) -> Result<BridgeIdentity, AppError> {
        let path = self.dir.join(IDENTITY_FILE);
        if path.is_file() {
            return read_json(&path);
        }

        let identity = BridgeIdentity::create();
        write_private_json(&path, &identity)?;
        Ok(identity)
    }
}

#[derive(Clone)]
pub struct BridgeDeviceStore {
    dir: PathBuf,
}

impl BridgeDeviceStore {
    pub fn at_default_path() -> Self {
        Self {
            dir: crate::core::paths::bridge_config_dir(),
        }
    }

    #[cfg(test)]
    pub fn at_path(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn list(&self) -> Result<Vec<BridgeDevice>, AppError> {
        let path = self.dir.join(DEVICES_FILE);
        if !path.is_file() {
            return Ok(Vec::new());
        }
        read_json(&path)
    }

    pub fn upsert(&self, device: BridgeDevice) -> Result<(), AppError> {
        let mut devices = self.list()?;
        if let Some(existing) = devices
            .iter_mut()
            .find(|candidate| candidate.device_id == device.device_id)
        {
            *existing = device;
        } else {
            devices.push(device);
        }
        write_private_json(&self.dir.join(DEVICES_FILE), &devices)
    }

    pub fn revoke(&self, device_id: &str) -> Result<bool, AppError> {
        let mut devices = self.list()?;
        let Some(device) = devices
            .iter_mut()
            .find(|candidate| candidate.device_id == device_id)
        else {
            return Ok(false);
        };
        device.revoked = true;
        write_private_json(&self.dir.join(DEVICES_FILE), &devices)?;
        Ok(true)
    }

    pub fn mark_seen(&self, device_id: &str) -> Result<bool, AppError> {
        let mut devices = self.list()?;
        let Some(device) = devices
            .iter_mut()
            .find(|candidate| candidate.device_id == device_id && !candidate.revoked)
        else {
            return Ok(false);
        };
        device.last_seen_at = Some(Utc::now());
        write_private_json(&self.dir.join(DEVICES_FILE), &devices)?;
        Ok(true)
    }
}

#[derive(Clone)]
pub struct BridgeClientIdentityStore {
    dir: PathBuf,
}

impl BridgeClientIdentityStore {
    /// Core paths are app-rooted before any store is built (Android sets
    /// `APP_DIRECTORIES` in setup; iOS/desktop use their platform roots), so
    /// the default path is the app-private bridge directory on every target.
    pub fn at_default_path() -> Self {
        Self {
            dir: crate::core::paths::bridge_config_dir(),
        }
    }

    pub fn at_app_path(app: &tauri::AppHandle) -> Result<Self, AppError> {
        Ok(Self {
            dir: client_config_dir(app)?,
        })
    }

    #[cfg(test)]
    pub fn at_path(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn load_or_create(&self) -> Result<BridgeClientIdentity, AppError> {
        self.load_file(true)
    }

    fn load_file(&self, create: bool) -> Result<BridgeClientIdentity, AppError> {
        with_file_mutation(|| {
            if let Some(raw) = read_optional(&self.dir.join(CLIENT_IDENTITY_FILE))? {
                let identity: BridgeClientIdentity = serde_json::from_slice(&raw)
                    .map_err(|_| IdentityFailure::Invalid.app_error())?;
                // Desktop's explicit Pair preserves the established v1 upgrade.
                // Resume and mobile migration never rotate an invalid/old signer.
                if identity.v != 1 || !create {
                    return validate_client_identity(identity).map_err(IdentityFailure::app_error);
                }
            } else if !create {
                return Err(IdentityFailure::Missing.app_error());
            }
            let identity = BridgeClientIdentity::create();
            write_private_json(&self.dir.join(CLIENT_IDENTITY_FILE), &identity)?;
            Ok(identity)
        })
    }
}

fn validate_client_identity(
    identity: BridgeClientIdentity,
) -> Result<BridgeClientIdentity, IdentityFailure> {
    if identity.v != 2 || identity.device_id.trim().is_empty() {
        return Err(IdentityFailure::Invalid);
    }
    let signing = identity
        .signing_key()
        .map_err(|_| IdentityFailure::Invalid)?;
    let verifying = identity
        .verifying_key()
        .map_err(|_| IdentityFailure::Invalid)?;
    if signing.verifying_key().to_bytes() != verifying.to_bytes() {
        return Err(IdentityFailure::Invalid);
    }
    Ok(identity)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum IdentityFailure {
    Missing,
    Invalid,
    Locked,
    Unavailable,
    CleanupPending,
}
impl IdentityFailure {
    pub(super) fn app_error(self) -> AppError {
        let (code, message) = match self {
            Self::Missing => (
                "bridge_identity_missing",
                "The paired Bridge signing identity is missing",
            ),
            Self::Invalid => (
                "bridge_identity_invalid",
                "The saved Bridge signing identity is invalid; recovery is required",
            ),
            Self::Locked => (
                "bridge_identity_locked",
                "Unlock the system credential store and try again",
            ),
            Self::Unavailable => (
                "bridge_identity_unavailable",
                "The system credential store is unavailable",
            ),
            Self::CleanupPending => (
                "bridge_identity_cleanup_pending",
                "Bridge identity is safely stored; legacy cleanup must be retried",
            ),
        };
        AppError::domain(code, message)
    }
}
impl From<CredentialError> for IdentityFailure {
    fn from(error: CredentialError) -> Self {
        match error {
            CredentialError::Locked => Self::Locked,
            CredentialError::Unavailable => Self::Unavailable,
            CredentialError::InvalidRecord | CredentialError::InvalidKey => Self::Invalid,
        }
    }
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, AppError> {
    match fs::read(path) {
        Ok(raw) => Ok(Some(raw)),
        Err(error)
            if error.kind() == std::io::ErrorKind::NotFound
                && fs::symlink_metadata(path).is_err() =>
        {
            Ok(None)
        }
        Err(error) => Err(error.into()),
    }
}

const CLIENT_RECORD_KEY: &str = "bridge.client-identity.v2";
// Rust-only envelope. Preserve the exact original JSON, including timestamp and
// key encodings; neither it nor its secret fields become an IPC payload.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SecureClientIdentity {
    v: u8,
    identity_json: Option<String>,
    cleared: bool,
    cleanup_complete: bool,
}
impl SecureClientIdentity {
    fn active(raw: String) -> Self {
        Self {
            v: 2,
            identity_json: Some(raw),
            cleared: false,
            cleanup_complete: true,
        }
    }
    fn cleared(complete: bool) -> Self {
        Self {
            v: 2,
            identity_json: None,
            cleared: true,
            cleanup_complete: complete,
        }
    }
    fn validate(&self) -> Result<(), IdentityFailure> {
        if self.v != 2
            || (self.cleared && self.identity_json.is_some())
            || (!self.cleared && (!self.cleanup_complete || self.identity_json.is_none()))
        {
            return Err(IdentityFailure::Invalid);
        }
        Ok(())
    }
    fn parse(&self) -> Result<BridgeClientIdentity, IdentityFailure> {
        self.validate()?;
        if self.cleared {
            return Err(if self.cleanup_complete {
                IdentityFailure::Missing
            } else {
                IdentityFailure::CleanupPending
            });
        }
        parse_mobile_identity(
            self.identity_json
                .as_deref()
                .ok_or(IdentityFailure::Invalid)?
                .as_bytes(),
        )
    }
}
fn parse_mobile_identity(raw: &[u8]) -> Result<BridgeClientIdentity, IdentityFailure> {
    let value: serde_json::Value =
        serde_json::from_slice(raw).map_err(|_| IdentityFailure::Invalid)?;
    let object = value.as_object().ok_or(IdentityFailure::Invalid)?;
    let fields = ["v", "deviceId", "publicKeyB64", "secretKeyB64", "createdAt"];
    if object.len() != fields.len() || fields.iter().any(|key| !object.contains_key(*key)) {
        return Err(IdentityFailure::Invalid);
    }
    let identity = serde_json::from_slice(raw).map_err(|_| IdentityFailure::Invalid)?;
    validate_client_identity(identity)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum LegacySource {
    IosKeychain,
    Json,
}
#[derive(Clone)]
struct LegacySnapshot {
    source: LegacySource,
    raw: Vec<u8>,
}
pub(super) trait LegacyIdentityStorage: Send + Sync {
    fn read(&self, source: LegacySource) -> Result<Option<Vec<u8>>, IdentityFailure>;
    fn remove_matching(
        &self,
        source: LegacySource,
        expected: &[u8],
    ) -> Result<bool, IdentityFailure>;
}
struct LegacyFiles {
    dir: PathBuf,
}
impl LegacyIdentityStorage for LegacyFiles {
    fn read(&self, source: LegacySource) -> Result<Option<Vec<u8>>, IdentityFailure> {
        match source {
            LegacySource::Json => {
                with_file_mutation(|| read_optional(&self.dir.join(CLIENT_IDENTITY_FILE)))
                    .map_err(|_| IdentityFailure::Unavailable)
            }
            LegacySource::IosKeychain => legacy_keychain_read(),
        }
    }
    fn remove_matching(
        &self,
        source: LegacySource,
        expected: &[u8],
    ) -> Result<bool, IdentityFailure> {
        match source {
            LegacySource::Json => with_file_mutation(|| {
                let path = self.dir.join(CLIENT_IDENTITY_FILE);
                let current = read_optional(&path)?;
                if current.as_deref() != Some(expected) {
                    return Ok(false);
                }
                fs::remove_file(path)?;
                Ok(true)
            })
            .map_err(|_| IdentityFailure::Unavailable),
            LegacySource::IosKeychain => {
                if legacy_keychain_read()?.as_deref() != Some(expected) {
                    return Ok(false);
                }
                legacy_keychain_remove()?;
                Ok(true)
            }
        }
    }
}
#[cfg(target_os = "ios")]
fn legacy_keychain_read() -> Result<Option<Vec<u8>>, IdentityFailure> {
    use security_framework::passwords::get_generic_password;
    use security_framework_sys::base::errSecItemNotFound;
    match get_generic_password(IOS_KEYCHAIN_SERVICE, IOS_KEYCHAIN_ACCOUNT) {
        Ok(raw) => Ok(Some(raw)),
        Err(error) if error.code() == errSecItemNotFound => Ok(None),
        Err(error) if error.code() == ERR_SEC_INTERACTION_NOT_ALLOWED => {
            Err(IdentityFailure::Locked)
        }
        Err(_) => Err(IdentityFailure::Unavailable),
    }
}
#[cfg(not(target_os = "ios"))]
fn legacy_keychain_read() -> Result<Option<Vec<u8>>, IdentityFailure> {
    Ok(None)
}
#[cfg(target_os = "ios")]
fn legacy_keychain_remove() -> Result<(), IdentityFailure> {
    security_framework::passwords::delete_generic_password(
        IOS_KEYCHAIN_SERVICE,
        IOS_KEYCHAIN_ACCOUNT,
    )
    .map_err(|_| IdentityFailure::Unavailable)
}
#[cfg(not(target_os = "ios"))]
fn legacy_keychain_remove() -> Result<(), IdentityFailure> {
    Ok(())
}

pub(super) struct ClientIdentityRepository {
    store: Option<CredentialStore>,
    strict: bool,
    files: BridgeClientIdentityStore,
    legacy: Arc<dyn LegacyIdentityStorage>,
    pending: tokio::sync::Mutex<Option<(SecureClientIdentity, Option<LegacySnapshot>)>>,
}
impl ClientIdentityRepository {
    pub(super) fn new(store: Option<CredentialStore>) -> Self {
        let files = BridgeClientIdentityStore::at_default_path();
        Self {
            strict: store.is_some() || cfg!(any(target_os = "ios", target_os = "android")),
            store,
            legacy: Arc::new(LegacyFiles {
                dir: files.dir.clone(),
            }),
            files,
            pending: tokio::sync::Mutex::new(None),
        }
    }
    pub(super) async fn acquire(&self, create: bool) -> Result<BridgeClientIdentity, AppError> {
        if !self.strict {
            let files = self.files.clone();
            return tokio::task::spawn_blocking(move || files.load_file(create))
                .await
                .map_err(|_| IdentityFailure::Unavailable.app_error())?;
        }
        self.mobile_acquire(create)
            .await
            .map_err(IdentityFailure::app_error)
    }
    async fn read_legacy(
        &self,
        source: LegacySource,
    ) -> Result<Option<LegacySnapshot>, IdentityFailure> {
        let legacy = self.legacy.clone();
        tokio::task::spawn_blocking(move || {
            legacy
                .read(source)
                .map(|raw| raw.map(|raw| LegacySnapshot { source, raw }))
        })
        .await
        .map_err(|_| IdentityFailure::Unavailable)?
    }
    async fn clean_exact(&self, snapshot: LegacySnapshot) -> Result<(), IdentityFailure> {
        let legacy = self.legacy.clone();
        let removed = tokio::task::spawn_blocking(move || {
            legacy.remove_matching(snapshot.source, &snapshot.raw)
        })
        .await
        .map_err(|_| IdentityFailure::CleanupPending)?;
        if removed.unwrap_or(false) {
            Ok(())
        } else {
            Err(IdentityFailure::CleanupPending)
        }
    }
    async fn clean_matching(&self, identity: &BridgeClientIdentity) -> Result<(), IdentityFailure> {
        for source in [LegacySource::IosKeychain, LegacySource::Json] {
            let snapshot = self
                .read_legacy(source)
                .await
                .map_err(|_| IdentityFailure::CleanupPending)?;
            if let Some(snapshot) = snapshot {
                // A verified native record is authoritative. Corrupt/conflicting
                // legacy bytes are retained, never used to rotate the signer.
                if parse_mobile_identity(&snapshot.raw).ok().as_ref() == Some(identity) {
                    self.clean_exact(snapshot).await?;
                }
            }
        }
        Ok(())
    }
    async fn commit_pending(
        &self,
        pending: &mut Option<(SecureClientIdentity, Option<LegacySnapshot>)>,
    ) -> Result<SecureClientIdentity, IdentityFailure> {
        let store = self.store.as_ref().ok_or(IdentityFailure::Unavailable)?;
        let (record, legacy) = pending.as_ref().ok_or(IdentityFailure::Invalid)?.clone();
        record.validate()?;
        store
            .write(CLIENT_RECORD_KEY, &record)
            .await
            .map_err(IdentityFailure::from)?;
        let verified = store
            .read::<SecureClientIdentity>(CLIENT_RECORD_KEY)
            .await
            .map_err(IdentityFailure::from)?;
        if verified.as_ref() != Some(&record) {
            return Err(IdentityFailure::Unavailable);
        }
        *pending = None;
        if let Some(snapshot) = legacy {
            self.clean_exact(snapshot).await?;
        }
        Ok(record)
    }
    async fn mobile_acquire(&self, create: bool) -> Result<BridgeClientIdentity, IdentityFailure> {
        let mut pending = self.pending.lock().await;
        let store = self.store.as_ref().ok_or(IdentityFailure::Unavailable)?;
        // Unknown native outcomes retain and retry exactly the same candidate.
        let saved = if pending.is_some() {
            Some(self.commit_pending(&mut pending).await?)
        } else {
            store
                .read::<SecureClientIdentity>(CLIENT_RECORD_KEY)
                .await
                .map_err(IdentityFailure::from)?
        };
        let mut selected = None;
        if let Some(saved) = saved {
            saved.validate()?;
            if !saved.cleared {
                let identity = saved.parse()?;
                self.clean_matching(&identity).await?;
                return Ok(identity);
            }
            if !saved.cleanup_complete {
                return Err(IdentityFailure::CleanupPending);
            }
            if !create {
                return Err(IdentityFailure::Missing);
            }
            // A cleared marker never falls back to legacy, even after restart.
            for source in [LegacySource::IosKeychain, LegacySource::Json] {
                if self
                    .read_legacy(source)
                    .await
                    .map_err(|_| IdentityFailure::CleanupPending)?
                    .is_some()
                {
                    return Err(IdentityFailure::CleanupPending);
                }
            }
        } else {
            for source in [LegacySource::IosKeychain, LegacySource::Json] {
                if let Some(snapshot) = self.read_legacy(source).await? {
                    parse_mobile_identity(&snapshot.raw)?;
                    selected = Some(snapshot);
                    break;
                }
            }
        }
        let raw = if let Some(snapshot) = &selected {
            String::from_utf8(snapshot.raw.clone()).map_err(|_| IdentityFailure::Invalid)?
        } else {
            if !create {
                return Err(IdentityFailure::Missing);
            }
            serde_json::to_string(&BridgeClientIdentity::create())
                .map_err(|_| IdentityFailure::Invalid)?
        };
        *pending = Some((SecureClientIdentity::active(raw), selected));
        let record = self.commit_pending(&mut pending).await?;
        let identity = record.parse()?;
        self.clean_matching(&identity).await?;
        Ok(identity)
    }
    pub(super) async fn begin_reset(&self) -> Result<(), AppError> {
        let mut pending = self.pending.lock().await;
        // Explicit reset alone may replace malformed native records without a read.
        if !pending.as_ref().is_some_and(|(record, _)| record.cleared) {
            *pending = Some((SecureClientIdentity::cleared(false), None));
        }
        self.commit_pending(&mut pending)
            .await
            .map_err(IdentityFailure::app_error)?;
        Ok(())
    }
    pub(super) async fn finish_reset(
        &self,
        profiles: &BridgeClientProfileStore,
    ) -> Result<(), AppError> {
        let mut pending = self.pending.lock().await;
        for source in [LegacySource::IosKeychain, LegacySource::Json] {
            let snapshot = self
                .read_legacy(source)
                .await
                .map_err(|_| IdentityFailure::CleanupPending.app_error())?;
            if let Some(snapshot) = snapshot {
                self.clean_exact(snapshot)
                    .await
                    .map_err(IdentityFailure::app_error)?;
            }
        }
        let profiles = profiles.clone();
        tokio::task::spawn_blocking(move || profiles.clear())
            .await
            .map_err(|_| IdentityFailure::CleanupPending.app_error())?
            .map_err(|_| IdentityFailure::CleanupPending.app_error())?;
        // Completion is durable too; a partial/unknown reset cannot report success.
        *pending = Some((SecureClientIdentity::cleared(true), None));
        self.commit_pending(&mut pending)
            .await
            .map_err(IdentityFailure::app_error)?;
        Ok(())
    }
}

#[derive(Clone)]
pub struct BridgeClientProfileStore {
    dir: PathBuf,
}

impl BridgeClientProfileStore {
    pub fn at_default_path() -> Self {
        Self {
            dir: crate::core::paths::bridge_config_dir(),
        }
    }

    pub fn at_app_path(app: &tauri::AppHandle) -> Result<Self, AppError> {
        Ok(Self {
            dir: client_config_dir(app)?,
        })
    }

    #[cfg(test)]
    pub fn at_path(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn load(&self) -> Result<Option<BridgeClientProfile>, AppError> {
        self.snapshot()?
            .map(|raw| serde_json::from_slice(&raw).map_err(AppError::from))
            .transpose()
    }

    pub(super) fn snapshot(&self) -> Result<Option<Vec<u8>>, AppError> {
        with_file_mutation(|| read_optional(&self.dir.join(CLIENT_PROFILE_FILE)))
    }

    fn write_profile(&self, profile: &BridgeClientProfile) -> Result<(), AppError> {
        fs::create_dir_all(&self.dir)?;
        atomic_write_with(
            &self.dir.join(CLIENT_PROFILE_FILE),
            &serde_json::to_vec_pretty(profile)?,
            &AtomicOpts::OWNER_ONLY,
        )?;
        Ok(())
    }

    pub fn save(&self, profile: &BridgeClientProfile) -> Result<(), AppError> {
        with_file_mutation(|| self.write_profile(profile))
    }

    pub(super) fn save_if_snapshot(
        &self,
        expected: &Option<Vec<u8>>,
        profile: &BridgeClientProfile,
    ) -> Result<bool, AppError> {
        with_file_mutation(|| {
            if read_optional(&self.dir.join(CLIENT_PROFILE_FILE))? != *expected {
                return Ok(false);
            }
            self.write_profile(profile)?;
            Ok(true)
        })
    }

    fn clear(&self) -> Result<(), AppError> {
        with_file_mutation(|| {
            let path = self.dir.join(CLIENT_PROFILE_FILE);
            if read_optional(&path)?.is_some() {
                fs::remove_file(path)?;
            }
            Ok(())
        })
    }

    pub fn mark_paired(&self) -> Result<(), AppError> {
        with_file_mutation(|| {
            let raw = read_optional(&self.dir.join(CLIENT_PROFILE_FILE))?
                .ok_or_else(|| AppError::message("Bridge connection profile is missing"))?;
            let mut profile: BridgeClientProfile = serde_json::from_slice(&raw)?;
            profile.paired = true;
            self.write_profile(&profile)
        })
    }
}

fn decode_32(value: &str, label: &str) -> Result<[u8; 32], AppError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| AppError::message(format!("{label} is not valid base64url")))?;
    bytes
        .try_into()
        .map_err(|_| AppError::message(format!("{label} must contain 32 bytes")))
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, AppError> {
    let raw = fs::read_to_string(path)?;
    serde_json::from_str(&raw).map_err(AppError::from)
}

fn write_private_json<T: Serialize>(path: &Path, value: &T) -> Result<(), AppError> {
    let parent = path
        .parent()
        .ok_or_else(|| AppError::message("Bridge config path has no parent"))?;
    fs::create_dir_all(parent)?;
    let raw = serde_json::to_vec_pretty(value)?;
    fs::write(path, raw)?;

    // iOS owns the app container and may reject chmod even for files created
    // by the app. The container already provides the required isolation;
    // keep explicit 0600 hardening for desktop Unix targets.
    #[cfg(all(unix, not(target_os = "ios")))]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "identity_tests.rs"]
pub(super) mod mobile_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir() -> PathBuf {
        std::env::temp_dir().join(format!("agentero-bridge-test-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn identity_is_stable_after_its_first_creation() {
        let dir = test_dir();
        let store = BridgeIdentityStore::at_path(dir.clone());
        let first = store.load_or_create().expect("create identity");
        let second = store.load_or_create().expect("load identity");

        assert_eq!(first, second);
        assert!(first.server_id.starts_with("agt_"));
        assert_eq!(
            first.public_key().expect("public key"),
            first.secret_key().expect("secret key").public_key()
        );

        fs::remove_dir_all(dir).expect("clean test directory");
    }

    #[test]
    fn device_registry_can_revoke_a_single_device() {
        let dir = test_dir();
        let store = BridgeDeviceStore::at_path(dir.clone());
        let device = BridgeDevice {
            device_id: "ios_1".to_string(),
            name: "Phil's iPhone".to_string(),
            public_key_b64: URL_SAFE_NO_PAD.encode([7_u8; 32]),
            paired_at: Utc::now(),
            last_seen_at: None,
            revoked: false,
        };
        store.upsert(device).expect("store device");

        assert!(store.revoke("ios_1").expect("revoke device"));
        assert!(store.list().expect("list devices")[0].revoked);

        fs::remove_dir_all(dir).expect("clean test directory");
    }

    #[test]
    fn client_identity_signs_a_challenge() {
        let identity = BridgeClientIdentity::create();
        let nonce = b"relay-connection-nonce";
        let signature = identity.sign_challenge(nonce).expect("sign challenge");
        verify_device_challenge(&identity.public_key_b64, nonce, &signature)
            .expect("verify challenge");
        assert!(verify_device_challenge(&identity.public_key_b64, b"wrong", &signature).is_err());
        assert_eq!(
            identity
                .verifying_key()
                .expect("valid public key")
                .to_bytes(),
            identity
                .signing_key()
                .expect("valid private key")
                .verifying_key()
                .to_bytes()
        );
    }

    #[test]
    fn old_client_identity_is_replaced_before_pairing() {
        let dir = test_dir();
        let store = BridgeClientIdentityStore::at_path(dir.clone());
        let old = BridgeClientIdentity {
            v: 1,
            device_id: "ios_old".to_string(),
            public_key_b64: URL_SAFE_NO_PAD.encode([1_u8; 32]),
            secret_key_b64: URL_SAFE_NO_PAD.encode([2_u8; 32]),
            created_at: Utc::now(),
        };
        write_private_json(&dir.join(CLIENT_IDENTITY_FILE), &old).expect("write old identity");

        let current = store.load_or_create().expect("replace old identity");
        assert_eq!(current.v, 2);
        assert_ne!(current.device_id, old.device_id);
        assert_eq!(
            current
                .signing_key()
                .expect("signing key")
                .verifying_key()
                .to_bytes(),
            current.verifying_key().expect("verifying key").to_bytes()
        );

        fs::remove_dir_all(dir).expect("clean test directory");
    }

    #[test]
    fn client_profile_persists_pairing_state() {
        let dir = test_dir();
        let store = BridgeClientProfileStore::at_path(dir.clone());
        store
            .save(&BridgeClientProfile {
                offer_url: "agentero://pair#offer=example".to_string(),
                device_name: "iPhone".to_string(),
                paired: false,
            })
            .expect("save profile");
        store.mark_paired().expect("mark profile paired");
        assert!(store.load().expect("load profile").expect("profile").paired);
        fs::remove_dir_all(dir).expect("clean test directory");
    }
}
