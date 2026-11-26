//! Local download policy and materialization history. This state never travels
//! in a remote manifest: an absent deferred asset is not a user deletion.

use crate::error::AppError;
use crate::features::sync::config::{SyncBackendConfig, SyncBackendKind};
use crate::features::sync::engine::{self, Progress, SyncOutcome};
use crate::features::sync::local;
use crate::features::sync::lock::SyncLock;
use crate::features::sync::snapshot::{self, FileEntry};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum DownloadMode {
    #[default]
    Full,
    OnDemand,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DownloadPolicy {
    pub mode: DownloadMode,
    pub pinned_paper_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum PaperAssetKind {
    Pdf,
    Source,
    Attachment,
    /// Derived finished translation; bulky but rebuildable from the source
    /// PDF + translation sidecar, so it follows the same on-demand policy.
    Translated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct PaperAssetAvailability {
    pub path: String,
    pub kind: PaperAssetKind,
    pub size: u64,
    pub materialized: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct PaperAvailability {
    pub paper_id: String,
    pub pinned: bool,
    pub assets: Vec<PaperAssetAvailability>,
}

const AVAILABILITY_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct AvailabilityState {
    pub version: u32,
    pub policy: DownloadPolicy,
    /// Positive history: these paths actually existed on this device. It is
    /// distinct from a base-manifest reference to a never-downloaded blob.
    pub materialized: BTreeSet<String>,
    /// Paper-level positive history survives cache eviction, but is pruned
    /// when the agreed manifest no longer contains that paper.
    pub materialized_papers: BTreeSet<String>,
    /// Explicit eviction or deferred download. Keep the base entry in merge
    /// until remote deletion or positive whole-paper deletion evidence.
    pub deferred: BTreeSet<String>,
}

impl Default for AvailabilityState {
    fn default() -> Self {
        Self {
            version: AVAILABILITY_VERSION,
            policy: DownloadPolicy::default(),
            materialized: BTreeSet::new(),
            materialized_papers: BTreeSet::new(),
            deferred: BTreeSet::new(),
        }
    }
}

pub fn read_policy(vault: &Path) -> Result<DownloadPolicy, AppError> {
    Ok(read_state(vault)?.policy)
}

/// Initialize a device preference without replacing an explicit or managed
/// policy. Desktop/CLI readers keep Full defaults; tablet hosts opt into lazy
/// downloads before their first pass. Lost managed ledgers still fail closed.
pub fn initialize_policy(
    vault: &Path,
    mut policy: DownloadPolicy,
) -> Result<DownloadPolicy, AppError> {
    if vault
        .join(".agentero/sync/availability.json")
        .try_exists()?
    {
        return read_policy(vault);
    }
    let _lock = acquire(vault)?;
    normalize_policy(&mut policy)?;
    let mut state = read_state(vault)?;
    if !vault
        .join(".agentero/sync/availability.json")
        .try_exists()?
    {
        state.policy = policy;
        write_state(vault, &state)?;
    }
    Ok(state.policy)
}

/// Policy changes affect the next pass; they never evict existing files.
pub fn set_policy(vault: &Path, mut policy: DownloadPolicy) -> Result<DownloadPolicy, AppError> {
    let _lock = acquire(vault)?;
    normalize_policy(&mut policy)?;
    let mut state = read_state(vault)?;
    state.policy = policy.clone();
    write_state(vault, &state)?;
    Ok(policy)
}

/// Offline inventory from the last successful manifest. `materialized` means a
/// local file is present; unsynced edits remain local files, not missing assets.
pub fn list_papers(vault: &Path) -> Result<Vec<PaperAvailability>, AppError> {
    let state = read_state(vault)?;
    let base = local::read_base(vault)?;
    engine::validate_manifest(&base)?;
    let mut papers = BTreeMap::new();
    for (path, entry) in &base.files {
        let Some((id, kind)) = asset(path) else {
            continue;
        };
        let paper = papers
            .entry(id.to_owned())
            .or_insert_with(|| PaperAvailability {
                paper_id: id.to_owned(),
                pinned: state.policy.pinned_paper_ids.iter().any(|p| p == id),
                assets: Vec::new(),
            });
        paper.assets.push(PaperAssetAvailability {
            path: path.clone(),
            kind,
            size: entry.size,
            materialized: vault.join(path).try_exists()? && vault.join(path).is_file(),
        });
    }
    Ok(papers.into_values().collect())
}

/// One-shot materialization without pinning. Uses the engine's identity check,
/// blob verification, merge/CAS retry, and cross-process vault lock. Category
/// exclusions still apply; metadata, notes, marks and layout always participate.
pub async fn fetch_paper(
    vault: &Path,
    cfg: &SyncBackendConfig,
    paper_id: &str,
    progress: Progress<'_>,
) -> Result<SyncOutcome, AppError> {
    let lock = acquire(vault)?;
    fetch_paper_locked(&lock, cfg, paper_id, progress).await
}

pub async fn fetch_paper_locked(
    lock: &SyncLock,
    cfg: &SyncBackendConfig,
    paper_id: &str,
    progress: Progress<'_>,
) -> Result<SyncOutcome, AppError> {
    validate_paper_id(paper_id)?;
    let vault = lock.vault();
    crate::features::sync::ensure_vault_sync_available(vault)?;
    if cfg.backend == SyncBackendKind::Git || engine::uses_webdav_journal(vault, cfg)? {
        return Err(AppError::domain(
            "sync_on_demand_unsupported",
            "This sync backend does not support on-demand paper downloads",
        ));
    }
    let result = engine::materialize_paper(vault, cfg, paper_id, progress).await;
    if let Err(error) = &result {
        local::record_error(vault, &error.to_string());
    }
    result
}

/// Evict only byte-verified synced bulky assets. Record deferral before removing
/// files so interruption cannot turn an eviction into a remote deletion. Notes,
/// marks, metadata, embedded assets and layout are never removed here.
pub fn evict_paper(vault: &Path, paper_id: &str) -> Result<usize, AppError> {
    validate_paper_id(paper_id)?;
    let _lock = acquire(vault)?;
    let mut state = read_state(vault)?;
    if state
        .policy
        .pinned_paper_ids
        .iter()
        .any(|id| id == paper_id)
    {
        return Err(AppError::domain(
            "sync_paper_pinned",
            "Unpin the paper before evicting its local assets",
        ));
    }
    let base = local::read_base(vault)?;
    engine::validate_manifest(&base)?;
    crate::fs::with_file_mutation(|| {
        let mut remove = Vec::new();
        for (path, entry) in &base.files {
            if asset(path).is_none_or(|(id, _)| id != paper_id) {
                continue;
            }
            ensure_unlinked(vault, path)?;
            match snapshot::hash_file(&vault.join(path)) {
                Ok(hash) if hash == entry.hash => remove.push(path.clone()),
                Ok(_) => {
                    return Err(AppError::domain(
                        "sync_asset_modified",
                        format!("{path} has unsynced changes"),
                    ))
                }
                Err(AppError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            state.deferred.insert(path.clone());
            state.materialized.remove(path);
        }
        // No physical removal precedes the durable deferral record.
        write_state(vault, &state)?;
        for path in &remove {
            fs::remove_file(vault.join(path))?;
        }
        Ok(remove.len())
    })
}

pub(super) fn acquire(vault: &Path) -> Result<SyncLock, AppError> {
    crate::features::sync::ensure_vault_sync_available(vault)?;
    SyncLock::try_acquire(vault)?.ok_or_else(|| {
        AppError::domain(
            "sync_busy",
            "another sync is already running for this vault",
        )
    })
}

pub(super) fn read_state(vault: &Path) -> Result<AvailabilityState, AppError> {
    let path = vault.join(".agentero/sync/availability.json");
    let raw = match fs::read(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if vault
                .join(".agentero/sync/availability.guard")
                .try_exists()?
            {
                return Err(AppError::message("sync availability ledger is missing; preserved download history requires repair"));
            }
            return Ok(AvailabilityState::default());
        }
        Err(e) => return Err(e.into()),
    };
    let mut state: AvailabilityState = serde_json::from_slice(&raw)
        .map_err(|e| AppError::message(format!("invalid sync availability state: {e}")))?;
    if state.version != AVAILABILITY_VERSION {
        return Err(AppError::message("unsupported sync availability version"));
    }
    normalize_policy(&mut state.policy)?;
    for id in &state.materialized_papers {
        // History comes from existing directory names, which may be broader
        // than the portable IDs accepted by new policy/fetch requests.
        if id.contains('/') || !safe_rel(id) {
            return Err(AppError::message(
                "unsafe paper history in sync availability state",
            ));
        }
    }
    for path in state.materialized.iter().chain(state.deferred.iter()) {
        if !safe_rel(path) {
            return Err(AppError::message("unsafe path in sync availability state"));
        }
    }
    if !state.materialized.is_disjoint(&state.deferred) {
        return Err(AppError::message(
            "sync availability state has contradictory history",
        ));
    }
    Ok(state)
}

pub(super) fn write_state(vault: &Path, state: &AvailabilityState) -> Result<(), AppError> {
    // A managed ledger disappearing is not a legacy full-download vault. Put
    // the durable guard first so an interrupted initial write fails closed.
    if !vault
        .join(".agentero/sync/availability.guard")
        .try_exists()?
    {
        local::write_sync_bytes(vault, "availability.guard", b"availability-v1\n")?;
    }
    local::write_sync_json(vault, "availability.json", state)
}

/// Retargeting retains local preferences, but no old remote's virtual entries or
/// positive deletion proof may participate in the new store's first merge.
pub(super) fn reset_history(vault: &Path, policy: DownloadPolicy) -> Result<(), AppError> {
    write_state(
        vault,
        &AvailabilityState {
            policy,
            ..Default::default()
        },
    )
}

pub(super) fn asset(path: &str) -> Option<(&str, PaperAssetKind)> {
    let (id, _) = path.strip_prefix("papers/")?.split_once('/')?;
    let kind = match snapshot::scope_category(path)? {
        "pdf" => PaperAssetKind::Pdf,
        "source" => PaperAssetKind::Source,
        "attachments" => PaperAssetKind::Attachment,
        "translation" => PaperAssetKind::Translated,
        _ => return None,
    };
    Some((id, kind))
}

pub(super) fn wants_download(policy: &DownloadPolicy, path: &str, requested: Option<&str>) -> bool {
    let Some((id, _)) = asset(path) else {
        return true;
    };
    policy.mode == DownloadMode::Full
        || requested == Some(id)
        || policy.pinned_paper_ids.iter().any(|p| p == id)
}

pub(super) fn validate_paper_id(id: &str) -> Result<(), AppError> {
    if id.trim().is_empty()
        || id == "."
        || id == ".."
        || id
            .chars()
            .any(|c| c == '/' || c == '\\' || c == ':' || c.is_control())
    {
        return Err(AppError::message("invalid paper ID"));
    }
    Ok(())
}

fn normalize_policy(policy: &mut DownloadPolicy) -> Result<(), AppError> {
    for id in &policy.pinned_paper_ids {
        validate_paper_id(id)?;
    }
    policy.pinned_paper_ids.sort();
    policy.pinned_paper_ids.dedup();
    Ok(())
}

fn safe_rel(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\\')
        && !path
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == "..")
        && crate::fs::safe_relative_path(path)
}

fn ensure_unlinked(vault: &Path, path: &str) -> Result<(), AppError> {
    let mut current = vault.to_path_buf();
    for component in path.split('/') {
        current.push(component);
        match current.symlink_metadata() {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(AppError::message("cannot evict assets through a symlink"))
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

/// Authoritative unfiltered removal ends a paper's materialization generation,
/// even when this device currently excludes its old assets from the scan. A
/// later remote paper with the same ID must start without stale deletion proof.
pub(super) fn reconcile_remote_history(state: &mut AvailabilityState, remote: &snapshot::Manifest) {
    if !remote.scope.is_all_for_paper_presence() {
        return;
    }
    let papers: BTreeSet<_> = remote
        .files
        .keys()
        .filter_map(|path| {
            path.strip_prefix("papers/")
                .and_then(|rest| rest.split_once('/'))
                .map(|(id, _)| id)
        })
        .collect();
    state
        .materialized_papers
        .retain(|id| papers.contains(id.as_str()));
    let retain = |path: &String| {
        path.strip_prefix("papers/")
            .and_then(|rest| rest.split_once('/'))
            .is_none_or(|(id, _)| papers.contains(id))
    };
    state.materialized.retain(retain);
    state.deferred.retain(retain);
}

/// Persist the physical/deferred distinction alongside each adopted base. Scope
/// exclusions retain old history; only observed manifest paths are reconciled.
pub(super) fn record_snapshot(
    vault: &Path,
    state: &mut AvailabilityState,
    merged: &BTreeMap<String, FileEntry>,
    deferred: &BTreeSet<String>,
    scope: &snapshot::SyncScope,
) -> Result<(), AppError> {
    state
        .materialized
        .retain(|path| snapshot::is_scope_excluded(scope, path));
    state
        .deferred
        .retain(|path| snapshot::is_scope_excluded(scope, path));
    for path in merged.keys() {
        if deferred.contains(path) {
            state.deferred.insert(path.clone());
        } else {
            state.materialized.insert(path.clone());
            if let Some((id, _)) = path
                .strip_prefix("papers/")
                .and_then(|rest| rest.split_once('/'))
            {
                state.materialized_papers.insert(id.to_owned());
            }
        }
    }
    let known_papers: BTreeSet<_> = merged
        .keys()
        .chain(state.materialized.iter())
        .chain(state.deferred.iter())
        .filter_map(|path| {
            path.strip_prefix("papers/")
                .and_then(|rest| rest.split_once('/'))
                .map(|(id, _)| id.to_owned())
        })
        .collect();
    let deferred_papers: BTreeSet<_> = deferred
        .iter()
        .filter_map(|path| {
            path.strip_prefix("papers/")
                .and_then(|rest| rest.split_once('/'))
                .map(|(id, _)| id)
        })
        .collect();
    let current_materialized: BTreeSet<_> = merged
        .keys()
        .filter(|path| !deferred.contains(*path))
        .filter_map(|path| {
            path.strip_prefix("papers/")
                .and_then(|rest| rest.split_once('/'))
                .map(|(id, _)| id)
        })
        .collect();
    state.materialized_papers.retain(|id| {
        known_papers.contains(id)
            // A remote modification may win against local paper deletion.
            // If that winning paper remains entirely deferred with no root,
            // its new snapshot never acquired positive local deletion proof.
            && (!deferred_papers.contains(id.as_str())
                || current_materialized.contains(id.as_str())
                || vault.join("papers").join(id).is_dir())
    });
    write_state(vault, state)
}

#[cfg(test)]
#[path = "availability_tests.rs"]
mod tests;
