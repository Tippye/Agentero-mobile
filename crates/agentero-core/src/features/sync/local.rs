//! Local sync state under `.agentero/` (ignored by the vault watcher, so
//! writes here never loop back as file-change events).
//!
//! - `.agentero/vault.json` — durable vault identity (UUID)
//! - `.agentero/sync/base.json` — manifest of the last successful sync
//! - `.agentero/sync/state.json` — last sync time / version for status UI
//! - `.agentero/sync/pushed.jsonl` — blobs uploaded but not yet published
//! - `.agentero/sync/availability.json` — versioned local policy and physical/deferred history
//! - `.agentero/sync/availability.guard` — distinguishes missing managed state from legacy defaults

use crate::error::AppError;
use crate::features::sync::{
    availability,
    config::SyncBackendConfig,
    snapshot::{self, Manifest},
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VaultIdentity {
    id: String,
    created_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncMeta {
    #[serde(default)]
    pub needs_merge: bool,
    #[serde(default)]
    pub last_sync_at: Option<String>,
    #[serde(default)]
    pub last_version: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_key: Option<String>,
    /// Last failed pass's error message; cleared by the next success.
    /// Surfaced by the CLI `sync status` (the desktop reports it live via
    /// `sync:state` events instead).
    #[serde(default)]
    pub last_error: Option<String>,
}

/// Record a failed pass on top of the existing meta (keeps last_sync_at /
/// last_version from the last success). Best-effort: a broken state.json
/// must not mask the sync error itself.
pub fn record_error(vault: &Path, error: &str) {
    // Never turn corrupt binding metadata into a fresh, unbound legacy record.
    if let Ok(stored) = read_stored_meta(vault) {
        let mut meta = stored.meta;
        meta.last_error = Some(error.to_string());
        let _ = write_bound_meta(
            vault,
            &meta,
            stored.store_identity.as_deref(),
            stored.manifest_key.as_deref(),
        );
    }
}

fn identity_path(vault: &Path) -> PathBuf {
    vault.join(".agentero").join("vault.json")
}

pub(super) fn sync_dir(vault: &Path) -> PathBuf {
    vault.join(".agentero").join("sync")
}

pub(super) fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), AppError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    crate::fs::json_store(path, value)
}

pub(super) fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Option<T>, AppError> {
    match fs::read(path) {
        Ok(raw) => serde_json::from_slice(&raw)
            .map(Some)
            .map_err(|e| AppError::message(format!("invalid {}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Durable vault UUID, created on first use.
pub fn ensure_vault_id(vault: &Path) -> Result<String, AppError> {
    if let Some(identity) = read_json::<VaultIdentity>(&identity_path(vault))? {
        if !identity.id.trim().is_empty() {
            return Ok(identity.id);
        }
    }
    let identity = VaultIdentity {
        id: uuid::Uuid::new_v4().to_string(),
        created_at: crate::time::now_rfc3339_millis(),
    };
    write_json(&identity_path(vault), &identity)?;
    Ok(identity.id)
}

/// Adopt a remote store's vault id (first join of an existing remote).
pub fn set_vault_id(vault: &Path, id: &str) -> Result<(), AppError> {
    write_json(
        &identity_path(vault),
        &VaultIdentity {
            id: id.to_string(),
            created_at: crate::time::now_rfc3339_millis(),
        },
    )
}

pub fn read_base(vault: &Path) -> Result<Manifest, AppError> {
    Ok(read_json(&sync_dir(vault).join("base.json"))?.unwrap_or_default())
}

pub fn write_base(vault: &Path, manifest: &Manifest) -> Result<(), AppError> {
    write_sync_json(vault, "base.json", manifest)
}

pub(super) fn read_base_strict(vault: &Path) -> Result<Manifest, AppError> {
    match fs::read(sync_dir(vault).join("base.json")) {
        Ok(raw) => serde_json::from_slice(&raw)
            .map_err(|error| AppError::message(format!("invalid sync base: {error}"))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let meta = read_meta_strict(vault)?;
            if meta.last_sync_at.is_some() || meta.last_version > 0 {
                Err(AppError::message(
                    "sync base is missing for established store history",
                ))
            } else {
                Ok(Manifest::default())
            }
        }
        Err(error) => Err(error.into()),
    }
}

#[derive(Default)]
struct StoredMeta {
    meta: SyncMeta,
    store_identity: Option<String>,
    manifest_key: Option<String>,
}

fn read_stored_meta(vault: &Path) -> Result<StoredMeta, AppError> {
    let raw = match fs::read(sync_dir(vault).join("state.json")) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(StoredMeta::default())
        }
        Err(error) => return Err(error.into()),
    };
    let value: serde_json::Value = serde_json::from_slice(&raw)
        .map_err(|error| AppError::message(format!("invalid sync metadata: {error}")))?;
    // Every historical writer emitted lastVersion. Partial JSON is not evidence
    // that a previously bound base has become an unbound legacy vault.
    if !value.is_object() || value.get("lastVersion").is_none() {
        return Err(AppError::message("incomplete sync metadata"));
    }
    let store_identity = match value.get("storeIdentity") {
        None => None,
        Some(serde_json::Value::String(id))
            if id.len() == 64 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) =>
        {
            Some(id.clone())
        }
        Some(_) => return Err(AppError::message("invalid sync store identity")),
    };
    let manifest_key = match value.get("manifestKey") {
        None => None,
        Some(serde_json::Value::String(key))
            if key.starts_with("manifests/")
                && !key.contains('\\')
                && !key
                    .split('/')
                    .any(|part| part.is_empty() || part == "." || part == "..") =>
        {
            Some(key.clone())
        }
        Some(_) => return Err(AppError::message("invalid agreed sync manifest key")),
    };
    let meta = serde_json::from_value(value)
        .map_err(|error| AppError::message(format!("invalid sync metadata: {error}")))?;
    Ok(StoredMeta {
        meta,
        store_identity,
        manifest_key,
    })
}

pub fn read_meta(vault: &Path) -> Result<SyncMeta, AppError> {
    Ok(read_json(&sync_dir(vault).join("state.json"))?.unwrap_or_default())
}

pub(super) fn read_meta_strict(vault: &Path) -> Result<SyncMeta, AppError> {
    Ok(read_stored_meta(vault)?.meta)
}

fn write_bound_meta(
    vault: &Path,
    meta: &SyncMeta,
    identity: Option<&str>,
    manifest_key: Option<&str>,
) -> Result<(), AppError> {
    let mut value = serde_json::to_value(meta)?;
    if let Some(identity) = identity {
        value["storeIdentity"] = serde_json::Value::String(identity.to_owned());
    }
    if let Some(key) = manifest_key {
        value["manifestKey"] = serde_json::Value::String(key.to_owned());
    }
    write_sync_json(vault, "state.json", &value)
}

pub(super) fn read_agreed_meta(vault: &Path) -> Result<(SyncMeta, Option<String>), AppError> {
    let stored = read_stored_meta(vault)?;
    Ok((stored.meta, stored.manifest_key))
}

pub fn write_meta(vault: &Path, meta: &SyncMeta) -> Result<(), AppError> {
    let stored = read_stored_meta(vault)?;
    write_bound_meta(
        vault,
        meta,
        stored.store_identity.as_deref(),
        stored.manifest_key.as_deref(),
    )
}

pub(super) fn write_agreed_meta(
    vault: &Path,
    meta: &SyncMeta,
    manifest_key: &str,
) -> Result<(), AppError> {
    let stored = read_stored_meta(vault)?;
    write_bound_meta(
        vault,
        meta,
        stored.store_identity.as_deref(),
        Some(manifest_key),
    )
}

pub(super) fn store_identity(config: &SyncBackendConfig) -> String {
    let mut config = config.clone().normalized();
    super::config::hydrate_agentero(&mut config);
    snapshot::hash_bytes(config.remote_identity().as_bytes())
}

/// Bind an existing legacy base to its OLD configured destination before a new
/// configuration is published. Caller holds the vault SyncLock. Existing bindings
/// are authoritative and are never overwritten from the current configuration.
pub fn bind_configured_store(vault: &Path, old: &SyncBackendConfig) -> Result<(), AppError> {
    let stored = read_stored_meta(vault)?;
    if stored.store_identity.is_none() {
        // Without even a historical metadata record the base's origin is unknown;
        // let the engine conservatively rebase it instead of guessing an owner.
        if !sync_dir(vault).join("state.json").try_exists()? {
            return Ok(());
        }
        write_bound_meta(
            vault,
            &stored.meta,
            Some(&store_identity(old)),
            stored.manifest_key.as_deref(),
        )?;
    }
    Ok(())
}

/// Establish a base/store boundary before scanning or trusting an idle HEAD.
/// The old binding remains until BOTH preceding reset records are durable.
/// Interruption at either cut retries the reset; vault content is never removed.
pub(super) fn namespaced_store_identity(
    config: &SyncBackendConfig,
    namespace: Option<&str>,
) -> Result<String, AppError> {
    let identity = store_identity(config);
    match namespace {
        None => Ok(identity),
        Some(namespace) => Ok(snapshot::hash_bytes(&serde_json::to_vec(&(
            "agentero-sync-namespace-v1",
            identity,
            namespace,
        ))?)),
    }
}

pub(super) fn prepare_store(
    vault: &Path,
    config: &SyncBackendConfig,
    namespace: Option<&str>,
) -> Result<String, AppError> {
    let identity = namespaced_store_identity(config, namespace)?;
    let stored = read_stored_meta(vault)?;
    if stored.store_identity.as_deref() != Some(identity.as_str()) {
        let state = availability::read_state(vault)?;
        write_base(vault, &Manifest::default())?;
        availability::reset_history(vault, state.policy)?;
        write_bound_meta(vault, &SyncMeta::default(), Some(&identity), None)?;
    }
    Ok(identity)
}

/// Critical sync-state publication: sync staged bytes before rename and the
/// containing Unix directories afterward, including newly created ancestors.
pub(super) fn write_sync_bytes(vault: &Path, name: &str, raw: &[u8]) -> Result<(), AppError> {
    let dir = sync_dir(vault);
    fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    for ancestor in [vault.to_path_buf(), vault.join(".agentero"), dir.clone()] {
        fs::File::open(ancestor)?.sync_all()?;
    }
    crate::fs::atomic_write_with(
        &dir.join(name),
        raw,
        &crate::fs::AtomicOpts {
            sync_file: true,
            ..Default::default()
        },
    )?;
    #[cfg(unix)]
    fs::File::open(&dir)?.sync_all()?;
    Ok(())
}

pub(super) fn write_sync_json<T: Serialize>(
    vault: &Path,
    name: &str,
    value: &T,
) -> Result<(), AppError> {
    write_sync_bytes(vault, name, &serde_json::to_vec_pretty(value)?)
}

/// Forget local sync state (disconnect). Keeps the vault identity.
/// Public so the Host integration layer can clear state after resolving
/// protected credentials on mobile.
pub fn clear(lock: &super::lock::SyncLock) -> Result<(), AppError> {
    let vault = lock.vault();
    clear_staged(vault)?;
    super::conflicts::clear(vault)?;
    for name in [
        "base.json",
        "state.json",
        "pushed.jsonl",
        "pending.json",
        "accepted.json",
        "webdav-journal.json",
    ] {
        match fs::remove_file(sync_dir(vault).join(name)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
            _ => (),
        }
    }
    Ok(())
}

const STAGE_PREFIX: &str = "download-";

pub(super) fn stage_file(vault: &Path) -> Result<tempfile::NamedTempFile, AppError> {
    Ok(tempfile::Builder::new()
        .prefix(STAGE_PREFIX)
        .suffix(".tmp")
        .tempfile_in(sync_dir(vault))?)
}

pub(super) fn clear_staged(vault: &Path) -> Result<(), AppError> {
    let dir = sync_dir(vault);
    fs::create_dir_all(&dir)?;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(STAGE_PREFIX) && name.ends_with(".tmp"))
            && entry.file_type()?.is_file()
        {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

pub(super) fn mark_catalog_dirty(vault: &Path) -> Result<(), AppError> {
    let dir = sync_dir(vault);
    fs::create_dir_all(&dir)?;
    fs::write(dir.join("catalog.dirty"), [])?;
    Ok(())
}

// ---- Pushed-blob log -------------------------------------------------------
//
// Blobs a pass uploaded but never got to publish, so the next pass can skip
// them instead of re-sending every byte on a conditional PUT. Appended per
// blob rather than written once at the
// end: a rate-limit error, a quit or an aborted task must not lose the record.
// The first line pins the remote store, because skipping an upload a different
// store never received would publish a manifest referencing missing blobs.

fn pushed_path(vault: &Path) -> PathBuf {
    sync_dir(vault).join("pushed.jsonl")
}

fn pushed_header(store_id: &str) -> String {
    format!("#{store_id}")
}

/// Hashes already uploaded to `store_id`. A log left by a different store is
/// removed rather than kept: appending under a header that no longer matches
/// would strand it, and the records are worthless for the new store anyway.
pub fn read_pushed(vault: &Path, store_id: &str) -> HashSet<String> {
    let path = pushed_path(vault);
    let Ok(raw) = fs::read_to_string(&path) else {
        return HashSet::new();
    };
    let mut lines = raw.lines();
    if lines.next() == Some(pushed_header(store_id).as_str()) {
        return lines
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect();
    }
    let _ = fs::remove_file(&path);
    HashSet::new()
}

/// Record one uploaded blob. Best-effort: a lost record only costs a redundant
/// upload, so a write failure must not fail the pass it is bookkeeping for.
pub fn record_pushed(vault: &Path, store_id: &str, hash: &str) {
    let path = pushed_path(vault);
    let result = (|| -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
        if file.metadata()?.len() == 0 {
            writeln!(file, "{}", pushed_header(store_id))?;
        }
        writeln!(file, "{hash}")
    })();
    if let Err(e) = result {
        log::warn!(target: "agentero::sync", "record pushed blob {hash}: {e}");
    }
}

/// Forget the log: a published manifest now covers those blobs, and it is the
/// authority every device consults.
pub fn clear_pushed(vault: &Path) {
    let _ = fs::remove_file(pushed_path(vault));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pushed_log_appends_dedupes_and_is_scoped_to_one_store() {
        let vault =
            std::env::temp_dir().join(format!("agentero-sync-local-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&vault).unwrap();

        assert!(read_pushed(&vault, "store-a").is_empty());
        record_pushed(&vault, "store-a", "h1");
        record_pushed(&vault, "store-a", "h2");
        // The same hash again after a second aborted pass.
        record_pushed(&vault, "store-a", "h1");
        assert_eq!(
            read_pushed(&vault, "store-a"),
            HashSet::from(["h1".to_string(), "h2".to_string()])
        );

        // Another store inherits nothing, and recording for it retires the
        // old log so the two cannot end up interleaved under one header.
        assert!(read_pushed(&vault, "store-b").is_empty());
        record_pushed(&vault, "store-b", "h3");
        assert_eq!(
            read_pushed(&vault, "store-b"),
            HashSet::from(["h3".to_string()])
        );
        assert!(read_pushed(&vault, "store-a").is_empty());

        clear_pushed(&vault);
        assert!(read_pushed(&vault, "store-b").is_empty());

        let _ = fs::remove_dir_all(&vault);
    }

    #[test]
    fn disconnect_also_forgets_the_pushed_log() {
        let vault =
            std::env::temp_dir().join(format!("agentero-sync-local-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&vault).unwrap();
        record_pushed(&vault, "store-a", "h1");
        let lock = super::super::lock::SyncLock::acquire(&vault).unwrap();
        clear(&lock).unwrap();
        assert!(super::super::lock::SyncLock::try_acquire(&vault)
            .unwrap()
            .is_none());
        drop(lock);
        assert!(read_pushed(&vault, "store-a").is_empty());
        let _ = fs::remove_dir_all(&vault);
    }
}
