//! Steam-style sync conflict resolution: when both devices edited the same
//! file, pause the whole pass and ask the user which version to keep instead
//! of silently applying mtime last-writer-wins.
//!
//! Two-phase protocol (the vault lock is never held while waiting for UI):
//! 1. A pass that finds divergent edits under `conflict_policy = "ask"` writes
//!    `.agentero/sync/pending-conflicts.json` and returns `sync_conflict_pending`
//!    without applying or publishing anything.
//! 2. The UI lists the conflicts, the user picks per file (keep local / keep
//!    cloud / keep both), and `sync_resolve_conflicts` persists the choices as
//!    an override file and re-runs the pass. The merge consumes the override,
//!    applies the chosen direction, and clears both files on success.
//!
//! Crash safety: both files are durable before the pass returns; a restart
//! re-reads them through `sync_get_status` so the dialog can resume. Until the
//! user decides, neither side is overwritten.

use crate::error::AppError;
use crate::features::sync::local;
use crate::features::sync::snapshot::FileEntry;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// One divergent-edit conflict surfaced to the user.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SyncFileConflict {
    /// Vault-relative path both devices edited.
    pub path: String,
    pub local_hash: String,
    pub local_size: u64,
    pub local_mtime_ms: i64,
    pub remote_hash: String,
    pub remote_size: u64,
    pub remote_mtime_ms: i64,
}

/// Durable record of a paused pass. Written before `sync_conflict_pending`
/// returns; removed once the resolved pass publishes successfully.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct PendingConflicts {
    /// Store identity the conflicts were detected against. A retargeted store
    /// invalidates the pending decision instead of applying it blindly.
    pub store_id: String,
    pub conflicts: Vec<SyncFileConflict>,
}

/// User's per-file decision. `keep_both` reproduces the historical automatic
/// behavior (newer wins the original path, older survives as a conflict copy).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum ConflictResolution {
    KeepLocal,
    KeepCloud,
    KeepBoth,
}

/// Durable overrides consumed by the next pass. Paths absent from the map fall
/// back to `default_resolution` so a partial UI submission is still complete.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictOverrides {
    pub store_id: String,
    pub default_resolution: ConflictResolution,
    pub resolutions: BTreeMap<String, ConflictResolution>,
}

fn pending_path(vault: &Path) -> std::path::PathBuf {
    local::sync_dir(vault).join("pending-conflicts.json")
}

fn overrides_path(vault: &Path) -> std::path::PathBuf {
    local::sync_dir(vault).join("conflict-overrides.json")
}

pub fn read_pending(vault: &Path) -> Result<Option<PendingConflicts>, AppError> {
    local::read_json(&pending_path(vault))
}

pub(super) fn write_pending(vault: &Path, pending: &PendingConflicts) -> Result<(), AppError> {
    local::write_json(&pending_path(vault), pending)
}

pub fn clear_pending(vault: &Path) -> Result<(), AppError> {
    let path = pending_path(vault);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(AppError::message(format!("clear pending conflicts: {e}"))),
    }
}

pub fn read_overrides(vault: &Path) -> Result<Option<ConflictOverrides>, AppError> {
    local::read_json(&overrides_path(vault))
}

pub fn write_overrides(vault: &Path, overrides: &ConflictOverrides) -> Result<(), AppError> {
    local::write_json(&overrides_path(vault), overrides)
}

pub fn clear_overrides(vault: &Path) -> Result<(), AppError> {
    let path = overrides_path(vault);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(AppError::message(format!("clear conflict overrides: {e}"))),
    }
}

/// Collect divergent-edit conflicts from a merge plan's inputs. Structured
/// marks (ink/annotations) merge losslessly by ID and never reach this path;
/// delete-vs-modify keeps the modification and is not a user decision either.
pub(super) fn detect_conflicts(
    base: &BTreeMap<String, FileEntry>,
    local: &BTreeMap<String, FileEntry>,
    remote: &BTreeMap<String, FileEntry>,
) -> Vec<SyncFileConflict> {
    let mut out = Vec::new();
    for (path, le) in local {
        let Some(re) = remote.get(path) else { continue };
        if le.hash == re.hash {
            continue;
        }
        // Only true three-way divergence: both sides changed from the base.
        // A missing base with differing content is a first-sync add/add race;
        // treat it as a conflict too so neither device silently loses bytes.
        if let Some(be) = base.get(path) {
            if be.hash == le.hash || be.hash == re.hash {
                continue;
            }
        }
        out.push(SyncFileConflict {
            path: path.clone(),
            local_hash: le.hash.clone(),
            local_size: le.size,
            local_mtime_ms: le.mtime_ms,
            remote_hash: re.hash.clone(),
            remote_size: re.size,
            remote_mtime_ms: re.mtime_ms,
        });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// Apply a user resolution to one divergent path inside the merge. Mirrors the
/// automatic keep-both branch but with an explicit winner; keep-local uploads
/// the local bytes, keep-cloud downloads the remote ones, and both variants
/// preserve the loser as a conflict copy so no data is destroyed.
pub(super) fn apply_resolution(
    plan: &mut super::engine::MergePlan,
    path: &str,
    local: &FileEntry,
    remote: &FileEntry,
    resolution: ConflictResolution,
    base: &BTreeMap<String, FileEntry>,
    remote_files: &BTreeMap<String, FileEntry>,
) {
    let keep_both_newer_remote = remote.mtime_ms >= local.mtime_ms;
    let (winner_is_remote, preserve_loser) = match resolution {
        ConflictResolution::KeepLocal => (false, true),
        ConflictResolution::KeepCloud => (true, true),
        ConflictResolution::KeepBoth => (keep_both_newer_remote, true),
    };
    let _ = preserve_loser;
    let loser = if winner_is_remote { local } else { remote };
    let copy = (1..)
        .map(|index| super::engine::conflict_name(path, &loser.hash, index))
        .find(|name| {
            ![base, remote_files, &plan.merged]
                .iter()
                .any(|files| files.contains_key(name))
        })
        .expect("unbounded conflict names");
    plan.conflict_copies.push(copy.clone());
    if winner_is_remote {
        plan.preserve_local.push((path.to_string(), copy.clone()));
        plan.merged.insert(copy, local.clone());
        plan.downloads.push((path.to_string(), remote.clone()));
        plan.merged.insert(path.to_string(), remote.clone());
    } else {
        plan.downloads.push((copy.clone(), remote.clone()));
        plan.merged.insert(copy, remote.clone());
        plan.merged.insert(path.to_string(), local.clone());
    }
}
