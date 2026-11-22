//! Durable review of ordinary files edited on both devices. A review and its
//! accepted choices share one atomic file, and the sync lock protects every
//! update. The engine verifies the complete reviewed set before applying it.

use crate::error::AppError;
use crate::features::sync::local;
use crate::features::sync::lock::SyncLock;
use crate::features::sync::snapshot::{self, FileEntry};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

const REVIEW_FILE: &str = "pending-conflicts.json";
const LEGACY_OVERRIDES_FILE: &str = "conflict-overrides.json";
pub const PENDING_CODE: &str = "sync_conflict_pending";

/// One divergent-edit conflict surfaced to the user.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SyncFileConflict {
    pub path: String,
    pub local_hash: String,
    pub local_size: u64,
    pub local_mtime_ms: i64,
    pub remote_hash: String,
    pub remote_size: u64,
    pub remote_mtime_ms: i64,
}

impl SyncFileConflict {
    pub(super) fn new(path: &str, local: &FileEntry, remote: &FileEntry) -> Self {
        Self {
            path: path.to_owned(),
            local_hash: local.hash.clone(),
            local_size: local.size,
            local_mtime_ms: local.mtime_ms,
            remote_hash: remote.hash.clone(),
            remote_size: remote.size,
            remote_mtime_ms: remote.mtime_ms,
        }
    }
}

/// Durable record of a paused pass. A review ID identifies the exact set of
/// local and cloud file versions presented for confirmation.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct PendingConflicts {
    pub store_id: String,
    pub review_id: String,
    pub conflicts: Vec<SyncFileConflict>,
}

/// Which file version takes the original path. The other version is preserved
/// as a conflict copy. `KeepBoth` uses the newer version at the original path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum ConflictResolution {
    KeepLocal,
    KeepCloud,
    KeepBoth,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Review {
    pub(super) store_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(super) review_id: String,
    pub(super) conflicts: Vec<SyncFileConflict>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) choices: Option<ConflictChoices>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ConflictChoices {
    default_resolution: ConflictResolution,
    resolutions: BTreeMap<String, ConflictResolution>,
}

impl ConflictChoices {
    pub(super) fn for_path(&self, path: &str) -> ConflictResolution {
        self.resolutions
            .get(path)
            .copied()
            .unwrap_or(self.default_resolution)
    }
}

fn versions(conflicts: &[SyncFileConflict]) -> BTreeMap<&str, (&str, &str)> {
    conflicts
        .iter()
        .map(|c| {
            (
                c.path.as_str(),
                (c.local_hash.as_str(), c.remote_hash.as_str()),
            )
        })
        .collect()
}

impl Review {
    pub(super) fn matches(&self, store_id: &str, conflicts: &[SyncFileConflict]) -> bool {
        self.store_id == store_id && versions(&self.conflicts) == versions(conflicts)
    }

    fn write(&self, vault: &Path) -> Result<(), AppError> {
        local::write_sync_json(vault, REVIEW_FILE, self)
    }
}

pub(super) fn read_review(vault: &Path) -> Result<Option<Review>, AppError> {
    let Some(mut review) = local::read_json::<Review>(&local::sync_dir(vault).join(REVIEW_FILE))?
    else {
        return Ok(None);
    };
    // Older clients persisted the shown versions but accepted unbound choices in
    // a separate file. The versions can be reviewed again under a stable ID.
    if review.review_id.is_empty() {
        review.choices = None;
        review.review_id = snapshot::hash_bytes(&serde_json::to_vec(&(
            "legacy-sync-review-v1",
            &review.store_id,
            versions(&review.conflicts),
        ))?);
    }
    Ok(Some(review))
}

pub fn read_pending(vault: &Path) -> Result<Option<PendingConflicts>, AppError> {
    Ok(read_review(vault)?
        .filter(|review| review.choices.is_none())
        .map(|review| PendingConflicts {
            store_id: review.store_id,
            review_id: review.review_id,
            conflicts: review.conflicts,
        }))
}

pub(super) fn pause(
    vault: &Path,
    store_id: &str,
    conflicts: Vec<SyncFileConflict>,
) -> Result<AppError, AppError> {
    Review {
        store_id: store_id.to_owned(),
        review_id: uuid::Uuid::new_v4().to_string(),
        conflicts,
        choices: None,
    }
    .write(vault)?;
    Ok(pending_error())
}

pub(super) fn pending_error() -> AppError {
    AppError::domain(
        PENDING_CODE,
        "Both devices edited the same files; choose which version to keep",
    )
}

pub(super) fn accept(
    lock: &SyncLock,
    review_id: &str,
    default_resolution: ConflictResolution,
    resolutions: BTreeMap<String, ConflictResolution>,
) -> Result<(), AppError> {
    let vault = lock.vault();
    let mut review = read_review(vault)?
        .filter(|review| review.review_id == review_id && review.choices.is_none())
        .ok_or_else(pending_error)?;
    let reviewed = versions(&review.conflicts);
    if let Some(path) = resolutions
        .keys()
        .find(|path| !reviewed.contains_key(path.as_str()))
    {
        return Err(AppError::message(format!(
            "{path} is not a pending sync conflict"
        )));
    }
    review.choices = Some(ConflictChoices {
        default_resolution,
        resolutions,
    });
    review.write(vault)
}

pub(super) fn clear(vault: &Path) -> Result<(), AppError> {
    clear_file(vault, LEGACY_OVERRIDES_FILE)?;
    clear_file(vault, REVIEW_FILE)
}

pub fn clear_locked(lock: &SyncLock) -> Result<(), AppError> {
    clear(lock.vault())
}

fn clear_file(vault: &Path, name: &str) -> Result<(), AppError> {
    let dir = local::sync_dir(vault);
    match std::fs::remove_file(dir.join(name)) {
        Ok(()) => {
            #[cfg(unix)]
            std::fs::File::open(&dir)?.sync_all()?;
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}
