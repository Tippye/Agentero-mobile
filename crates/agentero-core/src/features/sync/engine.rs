//! Sync engine: content-addressed blobs + immutable manifests + one mutable
//! `HEAD` pointer advanced with compare-and-swap.
//!
//! Remote layout under the configured prefix:
//!
//! ```text
//! vault.json                  { vaultId, formatVersion, encryption }
//! HEAD                        { version, manifestKey, updatedAt }   ← CAS
//! manifests/<v>-<nonce>.json.gz
//! blobs/<aa>/<sha256>         gzip(file content)
//! ```
//!
//! One pass = scan → three-way merge against the last synced manifest →
//! stage verified bytes → upload blobs and manifest → CAS `HEAD` → apply locally. A lost CAS re-runs the merge against the winner's manifest.
//!
//! Passes that would change nothing stop early: an up-to-date vault returns
//! after the single `HEAD` read, and a pass whose merged manifest matches the
//! remote one adopts it instead of publishing. Request-metered backends
//! (Nutstore: 600 per 30 min on a free plan) make the idle cost the dominant
//! one, and republishing mtime drift used to advance `HEAD` forever.
//!
//! A pass interrupted mid-upload resumes instead of starting over:
//! `.agentero/sync/pushed.jsonl` records the blobs it already sent, which no
//! manifest can vouch for until `HEAD` moves.

use crate::error::AppError;
use crate::features::sync::availability::{self, AvailabilityState, DownloadMode};
use crate::features::sync::config::{self, ConflictPolicy, SyncBackendConfig, SyncBackendKind};
use crate::features::sync::conflicts::{
    self, ConflictChoices, ConflictResolution, SyncFileConflict,
};
use crate::features::sync::local::{self, SyncMeta};
use crate::features::sync::scope;
use crate::features::sync::snapshot::{self, FileEntry, Manifest, SyncScope};
use crate::features::sync::store::{PutCondition, PutOutcome, RemoteStore, SyncStore};
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::io::{Read, Write};
use std::path::Path;

mod apply;
mod journal;
mod recovery;
use apply::prepare_local;
pub use journal::configure_webdav_journal;

pub fn uses_webdav_journal(vault: &Path, cfg: &SyncBackendConfig) -> Result<bool, AppError> {
    Ok(journal::binding(vault, cfg)?.is_some())
}

pub const FORMAT_VERSION: u32 = 1;
const HEAD_KEY: &str = "HEAD";
const VAULT_KEY: &str = "vault.json";
const MAX_CAS_RETRIES: usize = 5;
/// Decompressed manifest cap; a vault of 1M files serializes to ~150MB.
const MAX_MANIFEST_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteVaultInfo {
    vault_id: String,
    format_version: u32,
    encryption: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HeadPointer {
    version: u64,
    manifest_key: String,
    updated_at: String,
}

#[derive(Debug, Default, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SyncOutcome {
    pub version: u64,
    pub uploaded: usize,
    pub downloaded: usize,
    pub deleted_local: usize,
    pub removed_remote: usize,
    pub conflict_copies: Vec<String>,
}

/// `(phase, current, total)` — totals of 0 mean indeterminate.
pub type Progress<'a> = &'a (dyn Fn(&str, usize, usize) + Send + Sync);

/// Test the configured endpoint and its conditional-write semantics.
pub async fn test_connection(vault: &Path, cfg: &SyncBackendConfig) -> Result<bool, AppError> {
    crate::features::sync::ensure_vault_sync_available(vault)?;
    if let Some(state) = journal::binding(vault, cfg)? {
        let client = super::webdav::WebdavClient::new(cfg)?;
        state.verify_remote(&client).await?;
        client.test_journal().await?;
        return Ok(false);
    }
    if cfg.backend == SyncBackendKind::Git {
        crate::features::sync::git::test_connection(vault, cfg).await?;
        return Ok(true);
    }
    // The probe must measure the server, not echo the persisted verdict: a
    // config re-saved with a different address (or after the server's
    // behavior changed) must re-evaluate conditional-write support instead
    // of preserving a stale `conditional_writes = false`.
    let mut probe = cfg.clone();
    crate::features::sync::config::hydrate_agentero(&mut probe);
    let client = SyncStore::new(&probe, vault)?;
    client.ensure_root().await?;
    client.probe_conditional_writes().await
}

async fn sync_vault(
    vault: &Path,
    cfg: &SyncBackendConfig,
    progress: Progress<'_>,
) -> Result<SyncOutcome, AppError> {
    crate::features::sync::ensure_vault_sync_available(vault)?;
    if let Some(state) = journal::binding(vault, cfg)? {
        return journal::run(vault, cfg, state, progress).await;
    }
    if cfg.backend == SyncBackendKind::Git {
        if availability::read_policy(vault)?.mode == DownloadMode::OnDemand {
            return Err(AppError::domain(
                "sync_on_demand_unsupported",
                "Git sync does not support on-demand paper downloads",
            ));
        }
        if recovery::unfinished(vault)? {
            return Err(AppError::message(
                "unfinished object sync; restore its binding or disconnect explicitly",
            ));
        }
        return crate::features::sync::git::sync_vault(vault, cfg, progress).await;
    }
    let mut cfg = cfg.clone();
    crate::features::sync::config::hydrate_agentero(&mut cfg);
    let client = SyncStore::new(&cfg, vault)?;
    run_sync(vault, &cfg, &client, progress).await
}

/// One sync pass under the cross-process vault lock: the desktop Host, the
/// CLI and cron jobs on the same machine serialize here. Fails with the
/// `sync_busy` domain code while another process is mid-pass, and records
/// any failure into `.agentero/sync/state.json` (`last_error`) so
/// `sync status` can explain the last failed pass.
pub async fn sync_vault_locked(
    lock: &super::lock::SyncLock,
    cfg: &SyncBackendConfig,
    progress: Progress<'_>,
) -> Result<SyncOutcome, AppError> {
    let vault = lock.vault();
    match sync_vault(vault, cfg, progress).await {
        Ok(outcome) => Ok(outcome),
        Err(e) => {
            local::record_error(vault, &e.to_string());
            Err(e)
        }
    }
}

/// Confirm a specific review and continue with the vault lock still held.
pub async fn resolve_conflicts_locked(
    lock: &super::lock::SyncLock,
    cfg: &SyncBackendConfig,
    review_id: &str,
    default_resolution: ConflictResolution,
    resolutions: BTreeMap<String, ConflictResolution>,
    progress: Progress<'_>,
) -> Result<SyncOutcome, AppError> {
    if cfg.conflict_policy != ConflictPolicy::Ask
        || cfg.backend == SyncBackendKind::Git
        || journal::binding(lock.vault(), cfg)?.is_some()
    {
        return Err(conflicts::pending_error());
    }
    conflicts::accept(lock, review_id, default_resolution, resolutions)?;
    sync_vault_locked(lock, cfg, progress).await
}

/// Caller owns SyncLock; use the ordinary engine with a one-pass paper override.
pub(super) async fn materialize_paper(
    vault: &Path,
    cfg: &SyncBackendConfig,
    paper_id: &str,
    progress: Progress<'_>,
) -> Result<SyncOutcome, AppError> {
    let mut cfg = cfg.clone();
    crate::features::sync::config::hydrate_agentero(&mut cfg);
    let client = SyncStore::new(&cfg, vault)?;
    run_sync_requested(vault, &cfg, &client, progress, Some(paper_id)).await
}

/// Engine core, generic over the storage contract: `sync_vault` plugs in the
/// configured backend; tests can plug in an in-memory store instead.
async fn run_sync<S: RemoteStore>(
    vault: &Path,
    cfg: &SyncBackendConfig,
    client: &S,
    progress: Progress<'_>,
) -> Result<SyncOutcome, AppError> {
    run_sync_requested(vault, cfg, client, progress, None).await
}

async fn run_sync_requested<S: RemoteStore>(
    vault: &Path,
    cfg: &SyncBackendConfig,
    client: &S,
    progress: Progress<'_>,
    requested: Option<&str>,
) -> Result<SyncOutcome, AppError> {
    let ask = cfg.conflict_policy == ConflictPolicy::Ask;
    let review = if ask {
        conflicts::read_review(vault)?
    } else {
        None
    };
    if review
        .as_ref()
        .is_some_and(|review| review.choices.is_none())
    {
        return Err(conflicts::pending_error());
    }
    progress("scan", 0, 0);
    // Store ownership is established before hash-cache reuse, virtual deferrals,
    // idle detection, or remote IO. Retargeting never treats old history as a
    // deletion decision about the new destination.
    // Cloud namespace discovery may perform registration IO. Broken local
    // binding/base/availability evidence must fail before even that request.
    validate_manifest(&local::read_base_strict(vault)?)?;
    local::read_meta_strict(vault)?;
    availability::read_state(vault)?;
    let namespace = client.namespace_identity().await?;
    let store_id = local::prepare_store(vault, cfg, namespace.as_deref())?;
    // Pin the pushed-blob log and the recovery journal to this store, then
    // finish (or discard) any pass interrupted around its HEAD publication.
    local::clear_staged(vault)?;
    recovery::resume(vault, &store_id, client).await?;
    let mut review = if ask {
        conflicts::read_review(vault)?
    } else {
        None
    };
    let base = local::read_base_strict(vault)?;
    validate_manifest(&base)?;
    let mut asset_state = availability::read_state(vault)?;
    let (meta, agreed_manifest_key) = local::read_agreed_meta(vault)?;
    // Mobile retarget semantics: a base/meta version disagreement (e.g. another
    // metadata writer bumped state.json) is recovered by re-merging against the
    // remote HEAD below, not treated as a fatal checkpoint. The strict readers
    // above already reject corrupt/missing binding evidence before any IO.
    let base_scope = base.scope;
    let base_version = base.version;
    let base_had_files = !base.files.is_empty();
    // Symmetric filtering: the same predicate blinds the local scan and the
    // base, so excluded files are inert — never uploaded, downloaded, or
    // mistaken for deletions. The remote manifest stays unfiltered below.
    let mut base_files = visible_files(base.files.clone(), &cfg.scope);
    let scan_cache = if review.is_some() || meta.needs_merge {
        Manifest::default()
    } else {
        base
    };
    let mut physical_files = scan_local(vault, scan_cache, cfg.scope).await?;

    // A base reference is not proof this device ever cached the blob. Persist
    // actual observations, then virtualize only known/ambiguous deferrals.
    let mut observed = false;
    for path in physical_files.keys() {
        observed |= asset_state.materialized.insert(path.clone());
        observed |= asset_state.deferred.remove(path);
        if let Some(id) = paper_id(path) {
            observed |= asset_state.materialized_papers.insert(id.to_owned());
        }
    }
    if observed {
        availability::write_state(vault, &asset_state)?;
    }
    let local_papers = local_paper_directories(vault)?;
    let mut local_files =
        virtual_local_files(&base_files, &physical_files, &asset_state, &local_papers);
    let materialization_pending = base_files.keys().any(|path| {
        !physical_files.contains_key(path)
            && local_files.contains_key(path)
            && availability::wants_download(&asset_state.policy, path, requested)
    });
    let mut outcome = SyncOutcome::default();
    let mut ancestor_version = base_version;
    let mut ancestor_manifest_key = agreed_manifest_key.clone();
    // Opt-in degraded protocol (WebDAV only, phase 1): resolved from the live
    // probe on the first attempt, then stable across CAS retries.
    let mut degraded = false;
    for attempt in 0..MAX_CAS_RETRIES {
        if attempt > 0 {
            if ask {
                // Local apply already ran. Re-scan the actual bytes and keep the
                // agreed ancestor so a competing HEAD cannot inherit old choices.
                physical_files = scan_local(vault, Manifest::default(), cfg.scope).await?;
                let local_papers = local_paper_directories(vault)?;
                local_files =
                    virtual_local_files(&base_files, &physical_files, &asset_state, &local_papers);
            } else {
                for (path, entry) in &physical_files {
                    ensure_current(vault, path, Some(entry))?;
                }
            }
        }
        progress("pull", 0, 0);
        let head = client.get(HEAD_KEY).await?;
        if head.is_none()
            && (base_had_files
                || meta.last_sync_at.is_some()
                || meta.last_version > 0
                || ancestor_manifest_key.is_some())
        {
            // Deleting the last remote file publishes an empty manifest. Losing
            // HEAD is store damage even for an agreed empty base: restarting its
            // versions at one can make a lagging peer falsely consider itself idle.
            return Err(AppError::domain(
                "sync_remote_history_missing",
                "Remote sync history is missing; local files were left untouched",
            ));
        }
        let (head_ptr, head_etag) = match &head {
            Some((bytes, etag)) => (
                Some(serde_json::from_slice::<HeadPointer>(bytes)?),
                Some(etag.clone()),
            ),
            None => (None, None),
        };

        // A later attempt is a CAS retry, which always has work in flight.
        if attempt == 0 {
            // Up to date: HEAD still points at the manifest this vault last
            // agreed on, no local file changed content since, and this
            // device is using the same scope as before. One request
            // instead of a full pull/merge/publish pass — backends meter
            // requests per account (Nutstore allows 600 per 30 min on a free
            // plan), so the steady state has to be cheap. Sync history is
            // required because a first pass must establish the remote
            // identity; a no-op pass writes nothing, so skipping the
            // handshake below is what makes the single request possible.
            let up_to_date = !meta.needs_merge
                && requested.is_none()
                && meta.last_sync_at.is_some()
                && meta.store_id.as_deref() == Some(store_id.as_str())
                && base_version == meta.last_version
                && head_ptr.as_ref().is_some_and(|p| {
                    p.version == meta.last_version
                        && agreed_manifest_key.as_deref() == Some(p.manifest_key.as_str())
                })
                && base_scope == cfg.scope
                && !materialization_pending
                && same_content(&local_files, &base_files);
            if up_to_date {
                ensure_requested_asset(&base_files, &cfg.scope, requested)?;
                outcome.version = meta.last_version;
                // The pass succeeded (nothing to do); a stale failure note
                // from an earlier pass must not survive it.
                if meta.last_error.is_some() {
                    let mut cleared = meta.clone();
                    cleared.last_error = None;
                    local::write_meta(vault, &cleared)?;
                }
                conflicts::clear(vault)?;
                return Ok(outcome);
            }
            degraded = resolve_degraded(cfg, client).await?;
            ensure_remote_identity(vault, client, degraded).await?;
            // A new or reset store has no baseline against which it could
            // have deleted local files. Start by publishing the local scan.
            if head_ptr.is_none() {
                base_files.clear();
            }
        }

        // Conditional publication needs a usable strong ETag; the degraded
        // protocol takes the publish lock instead and ignores ETags entirely.
        if !degraded {
            if let Some(tag) = &head_etag {
                PutCondition::IfMatch(tag.clone()).header()?;
            }
        }
        // Keep the authoritative remote manifest intact. A device that skips
        // an asset carries its remote entry forward without transferring it.
        let remote_manifest = match &head_ptr {
            Some(ptr) => Some(load_manifest(client, ptr).await?),
            None => None,
        };
        let remote_scope = remote_manifest
            .as_ref()
            .map(|m| m.scope)
            .unwrap_or_default();
        if let Some(remote) = &remote_manifest {
            availability::reconcile_remote_history(&mut asset_state, remote);
        }
        let empty = BTreeMap::new();
        let remote_files = remote_manifest.as_ref().map_or(&empty, |m| &m.files);

        let paper_presence = PaperPresence {
            local: local_paper_directories(vault)?,
            remote: remote_manifest
                .as_ref()
                .map(|manifest| {
                    manifest
                        .files
                        .keys()
                        .filter_map(|path| paper_id(path).map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
        };
        // Plain-PUT stores can publish different snapshots with the same
        // counter (or restore an older HEAD). That is a fork, not evidence that
        // this branch's unchanged files were deliberately deleted by its peer.
        // Without a shared ancestor, merge conservatively as additions/edits
        // instead of inferring deletions: the fork still converges without
        // losing local data (stricter than a hard divergence stop).
        let fork_base = BTreeMap::new();
        let merge_base = if head_ptr.as_ref().is_some_and(|ptr| {
            (meta.last_sync_at.is_some() || attempt > 0)
                && ptr.version <= ancestor_version
                && ancestor_manifest_key.as_deref() != Some(ptr.manifest_key.as_str())
        }) {
            &fork_base
        } else {
            &base_files
        };
        let plan_for = |handling| {
            merge_with_paper_presence(
                merge_base,
                &local_files,
                remote_files,
                &cfg.scope,
                &remote_scope,
                &paper_presence,
                handling,
            )
        };
        let mut plan = plan_for(if ask {
            ConflictHandling::Review
        } else {
            ConflictHandling::Auto
        });
        if !plan.conflicts.is_empty() {
            let choices = review
                .as_ref()
                .filter(|review| review.matches(&store_id, &plan.conflicts))
                .and_then(|review| review.choices.as_ref());
            match choices {
                Some(choices) => plan = plan_for(ConflictHandling::Resolve(choices)),
                None => return Err(conflicts::pause(vault, &store_id, plan.conflicts)?),
            }
        } else if review.take().is_some() {
            // Once the reviewed set diverges, a later retry cannot resurrect it.
            conflicts::clear(vault)?;
        }
        resolve_marks(vault, client, &mut plan).await?;
        ensure_requested_asset(&plan.merged, &cfg.scope, requested)?;
        let deferred = plan_asset_downloads(
            &mut plan,
            &physical_files,
            &asset_state.policy,
            &cfg.scope,
            requested,
        );
        validate_tree(&plan.merged)?;
        // Stage verified bytes. Ink-durable paths publish durably here, before
        // any upload or HEAD publication: their rename must never outrun the
        // directory flush. Everything else applies only after the CAS wins.
        // Apply locally under the mutation lane BEFORE publishing (mobile
        // durability semantics): downloads/writes/deletes land durably first,
        // and a cooperating editor's concurrent save is rejected under the lane.
        // The recovery journal records intent before apply so a crash mid-apply
        // or mid-publish stays recoverable; upstream safety nets (journal, scope,
        // mandatory conditional writes, conservative fork merge) wrap this order.
        let merged = &plan.merged;
        let adopt = head_ptr
            .as_ref()
            .filter(|_| same_content(merged, remote_files));
        let accepted = if let Some(head) = adopt {
            recovery::save(
                vault,
                &store_id,
                head,
                &cfg.scope,
                &plan,
                &physical_files,
                true,
            )?;
            apply_local(
                vault,
                client,
                &plan,
                &physical_files,
                &mut outcome,
                progress,
            )
            .await?;
            head.clone()
        } else {
            progress("finalize", 0, 0);
            let new_version = head_ptr
                .as_ref()
                .map_or(0, |p| p.version)
                .checked_add(1)
                .ok_or_else(|| AppError::message("sync version exhausted"))?;
            let manifest_key = format!(
                "manifests/{new_version:010}-{}.json.gz",
                &uuid::Uuid::new_v4().simple().to_string()[..8]
            );
            let next_head = HeadPointer {
                version: new_version,
                manifest_key: manifest_key.clone(),
                updated_at: now(),
            };
            // Record intent before applying so a crash mid-apply is recoverable.
            recovery::save(
                vault,
                &store_id,
                &next_head,
                &cfg.scope,
                &plan,
                &physical_files,
                false,
            )?;
            // Apply and upload under the recorded intent; any failure before the
            // HEAD CAS discards the pending journal so recovery does not treat an
            // unpublished pass as accepted (local edits already landed durably
            // and re-merge on the next pass).
            if let Err(e) = apply_local(
                vault,
                client,
                &plan,
                &physical_files,
                &mut outcome,
                progress,
            )
            .await
            {
                recovery::discard(vault)?;
                return Err(e);
            }
            match plan
                .upload_missing(vault, &store_id, client, remote_files, &deferred, progress)
                .await
            {
                Ok(n) => outcome.uploaded += n,
                Err(e) => {
                    recovery::discard(vault)?;
                    return Err(e);
                }
            }
            let manifest = Manifest {
                version: new_version,
                files: merged.clone(),
                // Old clients could omit entries entirely. Keep that ambiguity
                // for absent paths while preserving every explicit remote entry.
                scope: remote_scope,
            };
            validate_manifest(&manifest)?;
            let manifest_put = async {
                client
                    .put(
                        &manifest_key,
                        gzip(&serde_json::to_vec(&manifest)?)?,
                        PutCondition::IfNoneMatch,
                    )
                    .await
            }
            .await;
            let manifest_put = match manifest_put {
                Ok(outcome) => outcome,
                Err(error) => {
                    recovery::discard(vault)?;
                    return Err(error);
                }
            };
            if manifest_put != PutOutcome::Ok {
                recovery::discard(vault)?;
                if !ask {
                    base_files = remote_files.clone();
                    local_files = merged.clone();
                    physical_files = merged
                        .iter()
                        .filter(|(path, _)| !deferred.contains(*path))
                        .map(|(path, entry)| (path.clone(), entry.clone()))
                        .collect();
                }
                continue;
            }
            let head_bytes = serde_json::to_vec(&next_head)?;
            let lost_race = if degraded {
                !publish_head_locked(vault, client, &head_bytes, head_ptr.as_ref()).await?
            } else {
                let condition = match head_etag {
                    Some(etag) => PutCondition::IfMatch(etag),
                    None => PutCondition::IfNoneMatch,
                };
                client.put(HEAD_KEY, head_bytes, condition).await? == PutOutcome::PreconditionFailed
            };
            if lost_race {
                recovery::discard(vault)?;
                if !ask {
                    base_files = remote_files.clone();
                    local_files = merged.clone();
                    physical_files = merged
                        .iter()
                        .filter(|(path, _)| !deferred.contains(*path))
                        .map(|(path, entry)| (path.clone(), entry.clone()))
                        .collect();
                }
                if !ask {
                    ancestor_version = head_ptr.as_ref().map_or(0, |ptr| ptr.version);
                    ancestor_manifest_key = head_ptr.as_ref().map(|ptr| ptr.manifest_key.clone());
                }
                continue;
            }
            recovery::accept(vault)?;
            next_head
        };
        outcome.removed_remote = remote_files
            .keys()
            .filter(|path| !merged.contains_key(*path))
            .count();
        availability::record_snapshot(
            vault,
            &mut asset_state,
            &plan.merged,
            &deferred,
            &cfg.scope,
        )?;
        record_sync(vault, &store_id, &accepted, &cfg.scope, plan.merged, false)?;
        // Keep the accepted journal until the review is retired. If cleanup is
        // interrupted, recovery can finish it before the next merge.
        conflicts::clear(vault)?;
        recovery::finish(vault)?;
        local::clear_pushed(vault);
        refresh_catalog_if_needed(vault)?;
        outcome.version = accepted.version;
        return Ok(outcome);
    }
    Err(AppError::message(
        "sync contention: too many concurrent updates, try again",
    ))
}

async fn scan_local(
    vault: &Path,
    cache: Manifest,
    scope: SyncScope,
) -> Result<BTreeMap<String, FileEntry>, AppError> {
    let dir = vault.to_owned();
    tokio::task::spawn_blocking(move || {
        crate::features::paper::catalog::sidecar::flush_pending(&dir)?;
        refresh_catalog_if_needed(&dir)?;
        snapshot::scan_vault(&dir, &cache, &scope)
    })
    .await
    .map_err(|e| AppError::message(format!("scan task: {e}")))?
}

/// Read-only impact preview for both overwrite directions.
pub async fn overwrite_preview(
    vault: &Path,
    cfg: &SyncBackendConfig,
    direction: OverwriteDirection,
) -> Result<OverwritePreview, AppError> {
    crate::features::sync::ensure_vault_sync_available(vault)?;
    ensure_webdav_overwrite(vault, cfg, direction)?;
    let client = SyncStore::new(cfg, vault)?;
    overwrite_preview_with(vault, cfg, &client, direction).await
}

fn ensure_webdav_overwrite(
    vault: &Path,
    cfg: &SyncBackendConfig,
    direction: OverwriteDirection,
) -> Result<(), AppError> {
    if cfg.backend != SyncBackendKind::Webdav || uses_webdav_journal(vault, cfg)? {
        return Err(AppError::message(
            "manual overwrite is only supported for WebDAV sync",
        ));
    }
    if cfg.conflict_policy == ConflictPolicy::Ask || conflicts::read_review(vault)?.is_some() {
        return Err(AppError::message(
            "manual overwrite is unavailable during conflict review",
        ));
    }
    if direction == OverwriteDirection::LocalToRemote {
        ensure_materialized_snapshot(vault, &cfg.scope)?;
    }
    Ok(())
}

async fn overwrite_preview_with<S: RemoteStore>(
    vault: &Path,
    cfg: &SyncBackendConfig,
    client: &S,
    direction: OverwriteDirection,
) -> Result<OverwritePreview, AppError> {
    let local_files = visible_files(scan_local_read_only(vault, cfg.scope).await?, &cfg.scope);
    let head = client.get(HEAD_KEY).await?;
    let head_ptr = match &head {
        Some((bytes, _)) => Some(serde_json::from_slice::<HeadPointer>(bytes)?),
        None => None,
    };
    if direction == OverwriteDirection::RemoteToLocal && head_ptr.is_none() {
        return Err(AppError::domain(
            "sync_remote_history_missing",
            "Remote sync history is missing; local files were left untouched",
        ));
    }
    let remote_manifest = match &head_ptr {
        Some(ptr) => Some(load_manifest(client, ptr).await?),
        None => None,
    };
    let empty = BTreeMap::new();
    let remote_files = remote_manifest.as_ref().map_or(&empty, |m| &m.files);
    let mut preview = OverwritePreview {
        remote_version: head_ptr.as_ref().map_or(0, |p| p.version),
        ..Default::default()
    };
    match direction {
        OverwriteDirection::RemoteToLocal => {
            let plan = plan_remote_mirror(&local_files, remote_files, &cfg.scope);
            preview.will_delete_local = plan.delete_local.len();
            preview.will_download = plan.downloads.len();
        }
        OverwriteDirection::LocalToRemote => {
            let remote_hashes: HashSet<&str> =
                remote_files.values().map(|e| e.hash.as_str()).collect();
            preview.will_upload = local_files
                .values()
                .filter(|e| !remote_hashes.contains(e.hash.as_str()))
                .count();
            preview.will_remove_remote = remote_files
                .keys()
                .filter(|path| {
                    !snapshot::is_scope_excluded(&cfg.scope, path)
                        && !local_files.contains_key(*path)
                })
                .count();
        }
    }
    Ok(preview)
}

/// Manual overwrite under the caller's SyncLock; failures are recorded into
/// `last_error` exactly like an ordinary pass.
pub async fn overwrite_locked(
    lock: &super::lock::SyncLock,
    cfg: &SyncBackendConfig,
    direction: OverwriteDirection,
    progress: Progress<'_>,
) -> Result<SyncOutcome, AppError> {
    let vault = lock.vault();
    let result = async {
        crate::features::sync::ensure_vault_sync_available(vault)?;
        ensure_webdav_overwrite(vault, cfg, direction)?;
        if recovery::unfinished(vault)? {
            return Err(AppError::message(
                "unfinished object sync; restore its binding or disconnect explicitly",
            ));
        }
        let client = SyncStore::new(cfg, vault)?;
        match direction {
            OverwriteDirection::RemoteToLocal => force_pull(vault, cfg, &client, progress).await,
            OverwriteDirection::LocalToRemote => force_push(vault, cfg, &client, progress).await,
        }
    }
    .await;
    match result {
        Ok(outcome) => Ok(outcome),
        Err(e) => {
            local::record_error(vault, &e.to_string());
            Err(e)
        }
    }
}

/// Remote overwrites local: mirror the remote HEAD manifest onto disk and
/// adopt it as the base. No new version is published. Divergent local
/// content is discarded by explicit user choice (confirmation dialog upstream).
async fn force_pull<S: RemoteStore>(
    vault: &Path,
    cfg: &SyncBackendConfig,
    client: &S,
    progress: Progress<'_>,
) -> Result<SyncOutcome, AppError> {
    validate_manifest(&local::read_base_strict(vault)?)?;
    local::read_meta_strict(vault)?;
    availability::read_state(vault)?;
    let store_id = local::prepare_store(vault, cfg, None)?;
    local::clear_staged(vault)?;
    progress("pull", 0, 0);
    let Some((head_bytes, _)) = client.get(HEAD_KEY).await? else {
        return Err(AppError::domain(
            "sync_remote_history_missing",
            "Remote sync history is missing; local files were left untouched",
        ));
    };
    let head_ptr: HeadPointer = serde_json::from_slice(&head_bytes)?;
    ensure_remote_identity(vault, client, false).await?;
    let remote_manifest = load_manifest(client, &head_ptr).await?;
    let scanned = scan_local(vault, Manifest::default(), cfg.scope).await?;
    let local_files = visible_files(scanned.clone(), &cfg.scope);
    let plan = plan_remote_mirror(&local_files, &remote_manifest.files, &cfg.scope);
    validate_tree(&plan.merged)?;
    let mut outcome = SyncOutcome::default();
    recovery::save(
        vault, &store_id, &head_ptr, &cfg.scope, &plan, &scanned, true,
    )?;
    apply_local(vault, client, &plan, &scanned, &mut outcome, progress).await?;
    let mut asset_state = availability::read_state(vault)?;
    availability::record_snapshot(
        vault,
        &mut asset_state,
        &plan.merged,
        &BTreeSet::new(),
        &cfg.scope,
    )?;
    record_sync(vault, &store_id, &head_ptr, &cfg.scope, plan.merged, false)?;
    recovery::finish(vault)?;
    local::clear_pushed(vault);
    refresh_catalog_if_needed(vault)?;
    outcome.version = head_ptr.version;
    Ok(outcome)
}

/// Local overwrites remote: publish the local scan as HEAD version + 1.
/// Remote-only entries drop out of the manifest; peers infer the deletions
/// through their ordinary three-way merge. Old manifests and blobs stay on
/// the store, so the superseded remote state remains recoverable until GC.
/// No local mutation happens, hence no staged apply or recovery journal.
async fn force_push<S: RemoteStore>(
    vault: &Path,
    cfg: &SyncBackendConfig,
    client: &S,
    progress: Progress<'_>,
) -> Result<SyncOutcome, AppError> {
    validate_manifest(&local::read_base_strict(vault)?)?;
    local::read_meta_strict(vault)?;
    availability::read_state(vault)?;
    let store_id = local::prepare_store(vault, cfg, None)?;
    local::clear_staged(vault)?;
    let degraded = resolve_degraded(cfg, client).await?;
    ensure_remote_identity(vault, client, degraded).await?;
    let scanned = scan_local(vault, Manifest::default(), cfg.scope).await?;
    let merged = visible_files(scanned, &cfg.scope);
    validate_tree(&merged)?;
    let mut outcome = SyncOutcome::default();
    let empty = BTreeMap::new();
    for attempt in 0..MAX_CAS_RETRIES {
        if attempt > 0 {
            progress("pull", 0, 0);
        }
        let head = client.get(HEAD_KEY).await?;
        let head_ptr = match &head {
            Some((bytes, _)) => Some(serde_json::from_slice::<HeadPointer>(bytes)?),
            None => None,
        };
        let remote_manifest = match &head_ptr {
            Some(ptr) => Some(load_manifest(client, ptr).await?),
            None => None,
        };
        let remote_files = remote_manifest.as_ref().map_or(&empty, |m| &m.files);
        let remote_scope = remote_manifest
            .as_ref()
            .map(|m| m.scope)
            .unwrap_or_default();
        let mut merged = merged.clone();
        merged.extend(
            remote_files
                .iter()
                .filter(|(path, _)| snapshot::is_scope_excluded(&cfg.scope, path))
                .map(|(path, entry)| (path.clone(), entry.clone())),
        );
        let plan = MergePlan {
            merged: merged.clone(),
            ..Default::default()
        };
        outcome.uploaded += plan
            .upload_missing(
                vault,
                &store_id,
                client,
                remote_files,
                &BTreeSet::new(),
                progress,
            )
            .await?;
        let new_version = head_ptr
            .as_ref()
            .map_or(0, |p| p.version)
            .checked_add(1)
            .ok_or_else(|| AppError::message("sync version exhausted"))?;
        let manifest_key = format!(
            "manifests/{new_version:010}-{}.json.gz",
            &uuid::Uuid::new_v4().simple().to_string()[..8]
        );
        let next_head = HeadPointer {
            version: new_version,
            manifest_key: manifest_key.clone(),
            updated_at: now(),
        };
        let manifest = Manifest {
            version: new_version,
            files: merged.clone(),
            // Keep the remote's scope declaration; this pass has no new
            // exclusions to announce beyond what the store already carries.
            scope: remote_scope,
        };
        validate_manifest(&manifest)?;
        if client
            .put(
                &manifest_key,
                gzip(&serde_json::to_vec(&manifest)?)?,
                PutCondition::IfNoneMatch,
            )
            .await?
            != PutOutcome::Ok
        {
            continue;
        }
        let head_bytes = serde_json::to_vec(&next_head)?;
        let lost_race = if degraded {
            !publish_head_locked(vault, client, &head_bytes, head_ptr.as_ref()).await?
        } else {
            let condition = match &head {
                Some((_, etag)) => PutCondition::IfMatch(etag.clone()),
                None => PutCondition::IfNoneMatch,
            };
            client.put(HEAD_KEY, head_bytes, condition).await? == PutOutcome::PreconditionFailed
        };
        if lost_race {
            continue;
        }
        outcome.removed_remote = remote_files
            .keys()
            .filter(|path| !merged.contains_key(*path))
            .count();
        record_sync(
            vault,
            &store_id,
            &next_head,
            &cfg.scope,
            merged.clone(),
            false,
        )?;
        local::clear_pushed(vault);
        outcome.version = new_version;
        return Ok(outcome);
    }
    Err(AppError::message(
        "sync contention: too many concurrent updates, try again",
    ))
}

/// Degraded-mode HEAD publication: acquire the publish lock, re-read HEAD
/// (a changed pointer rebases like a lost CAS), plain-PUT, read back, release.
/// `Ok(false)` means "lost the race or could not hold the lock" — the caller
/// rebases and retries; transport failures propagate as errors.
async fn publish_head_locked<S: RemoteStore>(
    vault: &Path,
    client: &S,
    head_bytes: &[u8],
    expected_head: Option<&HeadPointer>,
) -> Result<bool, AppError> {
    use crate::features::sync::store::LockOutcome;
    let device_id = local::ensure_device_id(vault)?;
    for attempt in 0..MAX_CAS_RETRIES {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(300 * attempt as u64)).await;
        }
        match client.try_acquire_publish_lock(&device_id).await? {
            LockOutcome::Acquired => {}
            LockOutcome::Busy => continue,
            LockOutcome::Unsupported => {
                return Err(AppError::message(
                    "degraded sync requires the WebDAV backend",
                ));
            }
        }
        let acquired = std::time::Instant::now();
        let result = async {
            let head = client.get(HEAD_KEY).await?;
            let current = match &head {
                Some((bytes, _)) => Some(serde_json::from_slice::<HeadPointer>(bytes)?),
                None => None,
            };
            let unchanged = match (&current, expected_head) {
                (Some(a), Some(b)) => a.version == b.version && a.manifest_key == b.manifest_key,
                (None, None) => true,
                _ => false,
            };
            if !unchanged {
                return Ok(false);
            }
            // The lock's server timestamp predates arbitration round-trips;
            // abort with half the TTL as margin so an expired lock can never
            // reach the publication (Joplin rule: own lock expired — cancel).
            if acquired.elapsed().as_secs() >= (super::store::PUBLISH_LOCK_TTL_SECS as u64) / 2 {
                return Ok(false);
            }
            client
                .put(HEAD_KEY, head_bytes.to_vec(), PutCondition::Unconditional)
                .await?;
            // Best-effort detection of a concurrent publisher that slipped
            // through arbitration; a mismatch rebases like a lost CAS.
            match client.get(HEAD_KEY).await? {
                Some((bytes, _)) if bytes == head_bytes => Ok(true),
                _ => Ok(false),
            }
        }
        .await;
        let _ = client.release_publish_lock(&device_id).await;
        return result;
    }
    Ok(false)
}

/// Which side wins a manual overwrite. `RemoteToLocal` mirrors the remote
/// HEAD manifest onto disk (discarding divergent local content);
/// `LocalToRemote` publishes the local scan as the new HEAD (superseding
/// remote changes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum OverwriteDirection {
    RemoteToLocal,
    LocalToRemote,
}

/// Impact preview for the confirmation dialog. Counts only; computed without
/// any local or remote write.
#[derive(Debug, Default, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct OverwritePreview {
    /// Local files the remote does not have (force pull deletes them).
    pub will_delete_local: usize,
    /// Remote files force pull downloads or restores over local edits.
    pub will_download: usize,
    /// Remote files absent locally (force push drops them from the manifest).
    pub will_remove_remote: usize,
    /// Blobs force push uploads (local content the remote lacks).
    pub will_upload: usize,
    pub remote_version: u64,
}

/// Resolve the degraded protocol from a live probe: supported store → false;
/// ignoring store + per-device opt-in on WebDAV → true; otherwise the
/// localized hard stop. Phase 1 keeps degraded sync WebDAV-only.
async fn resolve_degraded<S: RemoteStore>(
    cfg: &SyncBackendConfig,
    client: &S,
) -> Result<bool, AppError> {
    if client.probe_conditional_writes().await? {
        return Ok(false);
    }
    if cfg.allow_degraded_sync && cfg.backend == SyncBackendKind::Webdav {
        return Ok(true);
    }
    Err(AppError::domain(
        super::SYNC_NO_CONDITIONAL_WRITES,
        "remote does not enforce conditional writes; sync stopped without changing vault files",
    ))
}

/// Force-pull plan: remote membership wins wholesale within the sync scope.
/// Divergent local bytes are discarded (no conflict copies — the user chose
/// overwrite). `delete_local` is sorted because the staged apply resolves
/// directory replacements with a binary search.
fn plan_remote_mirror(
    local_files: &BTreeMap<String, FileEntry>,
    remote_files: &BTreeMap<String, FileEntry>,
    sync_scope: &SyncScope,
) -> MergePlan {
    let mut plan = MergePlan {
        merged: remote_files.clone(),
        ..Default::default()
    };
    // Scope-excluded categories stay inert: never downloaded, never deleted.
    let visible_remote = visible_files(remote_files.clone(), sync_scope);
    for (path, entry) in &visible_remote {
        if local_files
            .get(path)
            .is_some_and(|local| local.hash == entry.hash)
        {
            continue;
        }
        plan.downloads.push((path.clone(), entry.clone()));
    }
    for path in local_files.keys() {
        if !remote_files.contains_key(path) {
            plan.delete_local.push(path.clone());
        }
    }
    plan.delete_local.sort();
    plan.delete_local.dedup();
    plan.downloads.sort_by(|a, b| a.0.cmp(&b.0));
    plan
}

/// Read-only scan for the overwrite preview: never flushes pending sidecars
/// or refreshes the catalog.
async fn scan_local_read_only(
    vault: &Path,
    scope: SyncScope,
) -> Result<BTreeMap<String, FileEntry>, AppError> {
    let dir = vault.to_owned();
    let cache = local::read_base(vault)?;
    tokio::task::spawn_blocking(move || snapshot::scan_vault(&dir, &cache, &scope))
        .await
        .map_err(|e| AppError::message(format!("scan task: {e}")))?
}

fn record_sync(
    vault: &Path,
    store_id: &str,
    head: &HeadPointer,
    sync_scope: &SyncScope,
    files: BTreeMap<String, FileEntry>,
    needs_merge: bool,
) -> Result<(), AppError> {
    local::write_base(
        vault,
        &Manifest {
            version: head.version,
            files: visible_files(files, sync_scope),
            scope: *sync_scope,
        },
    )?;
    // Advance the agreed manifest key while preserving the durable store
    // binding (storeIdentity): a lost binding must never rebase history.
    local::write_agreed_meta(
        vault,
        &SyncMeta {
            needs_merge,
            last_sync_at: Some(now()),
            last_version: head.version,
            store_id: Some(store_id.to_owned()),
            manifest_key: Some(head.manifest_key.clone()),
            last_error: None,
        },
        &head.manifest_key,
    )
}

async fn load_manifest(client: &impl RemoteStore, ptr: &HeadPointer) -> Result<Manifest, AppError> {
    let name = ptr.manifest_key.strip_prefix("manifests/").filter(|name| {
        !name.is_empty() && !name.contains(['/', '\\', '?', '#', '%']) && name.ends_with(".json.gz")
    });
    if name.is_none() {
        return Err(AppError::message("invalid manifest key"));
    }
    let (bytes, _) = client
        .get(&ptr.manifest_key)
        .await?
        .ok_or_else(|| AppError::message(format!("manifest {} missing", ptr.manifest_key)))?;
    let manifest: Manifest = serde_json::from_slice(&gunzip_limited(&bytes, MAX_MANIFEST_BYTES)?)?;
    validate_manifest(&manifest)?;
    if manifest.version != ptr.version {
        return Err(AppError::message(
            "HEAD version does not match its manifest",
        ));
    }
    Ok(manifest)
}

/// First contact: create or verify the remote store identity.
/// A vault that never synced adopts an existing remote id (joining a store);
/// a vault with sync history refuses a foreign store.
async fn ensure_remote_identity<S: RemoteStore>(
    vault: &Path,
    client: &S,
    degraded: bool,
) -> Result<String, AppError> {
    let vault_id = local::ensure_vault_id(vault)?;
    match client.get(VAULT_KEY).await? {
        Some((bytes, _)) => {
            let info: RemoteVaultInfo = serde_json::from_slice(&bytes)
                .map_err(|_| AppError::message("remote vault.json is not a sync store"))?;
            if info.format_version > FORMAT_VERSION {
                return Err(AppError::message(
                    "remote sync store uses a newer format; update the app",
                ));
            }
            if info.vault_id == vault_id {
                return Ok(vault_id);
            }
            if local::read_base(vault)?.version == 0
                && local::read_meta(vault)?.last_sync_at.is_none()
            {
                local::set_vault_id(vault, &info.vault_id)?;
                return Ok(info.vault_id);
            }
            Err(AppError::message(
                "remote sync store belongs to a different vault",
            ))
        }
        None => {
            let info = RemoteVaultInfo {
                vault_id: vault_id.clone(),
                format_version: FORMAT_VERSION,
                encryption: "none".into(),
            };
            // Lost create race → re-verify against the winner.
            if degraded {
                // No create-if-absent guarantee on this store: plain PUT, then
                // read back so a foreign vault id is detected instead of
                // silently adopted.
                client
                    .put(
                        VAULT_KEY,
                        serde_json::to_vec_pretty(&info)?,
                        PutCondition::Unconditional,
                    )
                    .await?;
                let Some((bytes, _)) = client.get(VAULT_KEY).await? else {
                    return Err(AppError::message(
                        "remote vault.json vanished after creation",
                    ));
                };
                let stored: RemoteVaultInfo = serde_json::from_slice(&bytes)
                    .map_err(|_| AppError::message("remote vault.json is not a sync store"))?;
                if stored.vault_id != vault_id {
                    return Err(AppError::message(
                        "remote sync store belongs to a different vault",
                    ));
                }
                return Ok(vault_id);
            }
            match client
                .put(
                    VAULT_KEY,
                    serde_json::to_vec_pretty(&info)?,
                    PutCondition::IfNoneMatch,
                )
                .await?
            {
                PutOutcome::Ok => {
                    // Stores that ignore If-None-Match (Nutstore) let two
                    // concurrent creators both succeed; read back so a foreign
                    // vault id is detected instead of silently adopted.
                    let Some((bytes, _)) = client.get(VAULT_KEY).await? else {
                        return Err(AppError::message(
                            "remote vault.json vanished after creation",
                        ));
                    };
                    let stored: RemoteVaultInfo = serde_json::from_slice(&bytes)
                        .map_err(|_| AppError::message("remote vault.json is not a sync store"))?;
                    if stored.vault_id != vault_id {
                        return Err(AppError::message(
                            "remote sync store belongs to a different vault",
                        ));
                    }
                    Ok(vault_id)
                }
                PutOutcome::PreconditionFailed => {
                    Box::pin(ensure_remote_identity(vault, client, degraded)).await
                }
            }
        }
    }
}

/// Remote manifests are untrusted input: paths feed `vault.join` for
/// write/delete and hashes feed `blob_key`, so reject anything that could
/// escape the vault or index outside `blobs/`.
pub(super) fn validate_manifest(manifest: &Manifest) -> Result<(), AppError> {
    validate_tree(&manifest.files)?;
    for (rel, entry) in &manifest.files {
        if rel.is_empty()
            || rel.starts_with('/')
            || rel.contains('\\')
            || rel.split('/').any(|c| {
                c.is_empty()
                    || c == ".."
                    || c == "."
                    || snapshot::is_ignored_name(c)
                    || (cfg!(windows) && c.contains(':'))
            })
        {
            return Err(AppError::message(format!(
                "remote manifest has an unsafe path: {rel}"
            )));
        }
        if entry.hash.len() != 64
            || !entry
                .hash
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        {
            return Err(AppError::message(format!(
                "remote manifest has an invalid hash for {rel}"
            )));
        }
    }
    Ok(())
}

/// Only positive local history can distinguish paper removal from a directory
/// that never existed because every remote asset was deferred.
fn virtual_local_files(
    base: &BTreeMap<String, FileEntry>,
    physical: &BTreeMap<String, FileEntry>,
    state: &AvailabilityState,
    local_papers: &HashSet<String>,
) -> BTreeMap<String, FileEntry> {
    let deleted_papers: HashSet<&str> = base
        .keys()
        .filter_map(|path| {
            let id = paper_id(path)?;
            (!local_papers.contains(id)
                && (availability::asset(path).is_none()
                    || state.materialized.contains(path)
                    || state.materialized_papers.contains(id)))
            .then_some(id)
        })
        .collect();
    let mut effective = physical.clone();
    for (path, entry) in base {
        if availability::asset(path).is_none() || physical.contains_key(path) {
            continue;
        }
        if paper_id(path).is_some_and(|id| deleted_papers.contains(id)) {
            continue;
        }
        if state.deferred.contains(path)
            || (state.policy.mode == DownloadMode::OnDemand && !state.materialized.contains(path))
        {
            effective.insert(path.clone(), entry.clone());
        }
    }
    effective
}

fn ensure_requested_asset(
    files: &BTreeMap<String, FileEntry>,
    sync_scope: &SyncScope,
    requested: Option<&str>,
) -> Result<(), AppError> {
    if let Some(id) = requested {
        if !files.keys().any(|path| {
            !snapshot::is_scope_excluded(sync_scope, path)
                && availability::asset(path).is_some_and(|(paper, _)| paper == id)
        }) {
            return Err(AppError::domain(
                "sync_paper_unavailable",
                "The paper has no remote assets in the enabled sync categories",
            ));
        }
    }
    Ok(())
}

fn validate_tree<T>(files: &BTreeMap<String, T>) -> Result<(), AppError> {
    for path in files.keys() {
        if path
            .match_indices('/')
            .any(|(offset, _)| files.contains_key(&path[..offset]))
        {
            return Err(AppError::message(format!(
                "file/directory conflict at {path}; preserve both versions and rename one"
            )));
        }
    }
    Ok(())
}

/// Change only local materialization actions, never shared manifest membership.
/// Scope-excluded carry-forward entries stay inert: they are neither downloaded
/// nor deferred (the device never asked for that category).
fn plan_asset_downloads(
    plan: &mut MergePlan,
    physical: &BTreeMap<String, FileEntry>,
    policy: &availability::DownloadPolicy,
    sync_scope: &SyncScope,
    requested: Option<&str>,
) -> BTreeSet<String> {
    let mut deferred = BTreeSet::new();
    for (path, entry) in &plan.merged {
        let Some((paper, _)) = availability::asset(path) else {
            continue;
        };
        // An explicit fetch verifies the requested paper's blobs even when the
        // asset is already materialized, so a corrupt remote blob is caught.
        let forced = requested.is_some_and(|id| paper == id);
        if plan.conflict_copies.contains(path) || (physical.contains_key(path) && !forced) {
            continue;
        }
        if snapshot::is_scope_excluded(sync_scope, path) {
            continue;
        }
        if !availability::wants_download(policy, path, requested) {
            deferred.insert(path.clone());
        } else if !plan.downloads.iter().any(|(rel, _)| rel == path)
            && !plan.writes.contains_key(path)
        {
            plan.downloads.push((path.clone(), entry.clone()));
        }
    }
    plan.downloads.retain(|(path, _)| !deferred.contains(path));
    deferred
}

#[derive(Default)]
pub(super) struct MergePlan {
    pub(super) merged: BTreeMap<String, FileEntry>,
    pub(super) downloads: Vec<(String, FileEntry)>,
    pub(super) delete_local: Vec<String>,
    /// `(original rel, conflict-copy rel)` — copy the local file aside before
    /// the remote version overwrites it.
    pub(super) preserve_local: Vec<(String, String)>,
    pub(super) conflict_copies: Vec<String>,
    marks: Vec<MarkMerge>,
    /// Resolved semantic content and exact source conflict copies.
    pub(super) writes: BTreeMap<String, Vec<u8>>,
    pub(super) mark_conflicts: Vec<String>,
    conflicts: Vec<SyncFileConflict>,
}

impl MergePlan {
    async fn upload_missing(
        &self,
        vault: &Path,
        store_id: &str,
        client: &impl RemoteStore,
        remote_files: &BTreeMap<String, FileEntry>,
        deferred: &BTreeSet<String>,
        progress: Progress<'_>,
    ) -> Result<usize, AppError> {
        let mut uploaded = 0;
        let remote_hashes: HashSet<&str> = remote_files.values().map(|e| e.hash.as_str()).collect();
        let mut pushed = local::read_pushed(vault, store_id);
        let uploads: Vec<(&String, &FileEntry)> = self
            .merged
            .iter()
            // Deferred entries came from an agreed immutable remote blob;
            // their absence must neither upload local bytes nor delete them.
            .filter(|(path, e)| {
                !deferred.contains(*path) && !remote_hashes.contains(e.hash.as_str())
            })
            .collect();
        let total = uploads.len();
        for (i, (rel, entry)) in uploads.into_iter().enumerate() {
            progress("upload", i + 1, total);
            // Also covers two paths with identical content in one pass.
            if pushed.contains(&entry.hash) {
                continue;
            }
            // Local apply already installed conflict copies. Their original
            // paths may now contain the cloud winner, so read the copy itself.
            let blob = match self.writes.get(rel) {
                Some(bytes) => {
                    if snapshot::hash_bytes(bytes) != entry.hash {
                        return Err(AppError::message(format!(
                            "{rel} changed during sync; run sync again"
                        )));
                    }
                    gzip(bytes)?
                }
                None => read_blob(vault, rel, entry)?,
            };
            client
                .put(&blob_key(&entry.hash), blob, PutCondition::IfNoneMatch)
                .await?;
            pushed.insert(entry.hash.clone());
            local::record_pushed(vault, store_id, &entry.hash);
            uploaded += 1;
        }

        Ok(uploaded)
    }
}

fn visible_files(
    mut files: BTreeMap<String, FileEntry>,
    sync_scope: &SyncScope,
) -> BTreeMap<String, FileEntry> {
    if !sync_scope.is_all() {
        let roots = scope::paper_roots(files.keys().map(String::as_str));
        files.retain(|path, _| !scope::excluded(sync_scope, path, &roots));
    }
    files
}

struct MarkMerge {
    path: String,
    base: Option<FileEntry>,
    local: Option<FileEntry>,
    remote: Option<FileEntry>,
}

fn paper_id(path: &str) -> Option<&str> {
    path.strip_prefix("papers/")?
        .split_once('/')
        .map(|(id, _)| id)
}

struct PaperPresence {
    local: HashSet<String>,
    remote: HashSet<String>,
}

fn local_paper_directories(vault: &Path) -> Result<HashSet<String>, AppError> {
    let entries = match fs::read_dir(vault.join("papers")) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(HashSet::new()),
        Err(e) => return Err(e.into()),
    };
    let mut papers = HashSet::new();
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            if let Some(name) = entry.file_name().to_str() {
                papers.insert(name.to_owned());
            }
        }
    }
    Ok(papers)
}

#[derive(Clone, Copy)]
enum MarkKind {
    Annotations,
    Ink(u32),
    Other,
}

fn mark_kind(path: &str) -> Option<MarkKind> {
    let (_, tail) = path.strip_prefix("papers/")?.split_once('/')?;
    if tail == "annotations.json" || tail == "marks/annotations.json" {
        return Some(MarkKind::Annotations);
    }
    if let Some(page) = tail
        .strip_prefix("marks/ink/p")
        .and_then(|s| s.strip_suffix(".json"))
    {
        if !page.is_empty() && page.bytes().all(|c| c.is_ascii_digit()) {
            if let Ok(page) = page.parse::<u32>() {
                if page > 0 {
                    return Some(MarkKind::Ink(page));
                }
            }
        }
    }
    tail.starts_with("marks/").then_some(MarkKind::Other)
}

/// Three-way merge per path. Structured PDF marks are resolved from immutable
/// blobs before applying; ambiguous marks always retain exact conflict copies.
/// Markdown/table conflicts keep both files. Other binaries use mtime.
/// Delete-vs-modify keeps the modification at the file level.
///
/// A local exclusion preserves the remote entry without transferring bytes.
/// A legacy remote omission in a filtered category cannot delete local data.
fn merge(
    base: &BTreeMap<String, FileEntry>,
    local: &BTreeMap<String, FileEntry>,
    remote: &BTreeMap<String, FileEntry>,
    local_scope: &SyncScope,
    remote_scope: &SyncScope,
) -> MergePlan {
    let presence = PaperPresence {
        local: local
            .keys()
            .filter_map(|path| paper_id(path).map(str::to_owned))
            .collect(),
        remote: remote
            .keys()
            .filter_map(|path| paper_id(path).map(str::to_owned))
            .collect(),
    };
    merge_with_paper_presence(
        base,
        local,
        remote,
        local_scope,
        remote_scope,
        &presence,
        ConflictHandling::Auto,
    )
}

#[derive(Clone, Copy)]
enum ConflictHandling<'a> {
    Auto,
    Review,
    Resolve(&'a ConflictChoices),
}

fn merge_with_paper_presence(
    base: &BTreeMap<String, FileEntry>,
    local: &BTreeMap<String, FileEntry>,
    remote: &BTreeMap<String, FileEntry>,
    local_scope: &SyncScope,
    remote_scope: &SyncScope,
    paper_presence: &PaperPresence,
    handling: ConflictHandling<'_>,
) -> MergePlan {
    let mut plan = MergePlan::default();
    let mut paths: Vec<&String> = base
        .keys()
        .chain(local.keys())
        .chain(remote.keys())
        .collect();
    paths.sort();
    paths.dedup();
    let roots = if local_scope.is_all() && remote_scope.is_all() {
        Default::default()
    } else {
        scope::paper_roots(paths.iter().map(|path| path.as_str()))
    };

    for path in paths {
        let b = base.get(path);
        let l = local.get(path);
        let r = remote.get(path);

        if scope::excluded(local_scope, path, &roots) {
            if let Some(entry) = r {
                plan.merged.insert(path.clone(), entry.clone());
            }
            continue;
        }
        if r.is_none() && scope::excluded(remote_scope, path, &roots) {
            if let Some(entry) = l {
                plan.merged.insert(path.clone(), entry.clone());
            }
            continue;
        }

        let same = |a: Option<&FileEntry>, b: Option<&FileEntry>| match (a, b) {
            (Some(x), Some(y)) => x.hash == y.hash,
            (None, None) => true,
            _ => false,
        };

        // Ink omission never deletes records, including an entire missing
        // sidecar or equal peers that both omitted old tombstones. Only a
        // whole-paper removal may delete unchanged ink. Local directory and
        // unfiltered remote evidence must survive bulky-asset scope filtering.
        let kind = mark_kind(path);
        let structured = if matches!(kind, Some(MarkKind::Ink(_))) {
            let paper = paper_id(path).expect("ink belongs to a paper");
            let local_paper_deleted =
                b.is_some() && !paper_presence.local.contains(paper) && (r.is_none() || same(r, b));
            // A scoped publisher may hide the only surviving PDF/source. Its
            // absence cannot prove paper removal without an explicit marker.
            let remote_paper_deleted = b.is_some()
                && remote_scope.is_all_for_paper_presence()
                && !paper_presence.remote.contains(paper)
                && (l.is_none() || same(l, b));
            !local_paper_deleted
                && !remote_paper_deleted
                && (!same(l, b) || !same(r, b))
                && (b.is_some() || (l.is_some() && r.is_some() && !same(l, r)))
        } else {
            kind.is_some()
                && !same(l, r)
                && l.is_some()
                && r.is_some()
                && !same(l, b)
                && !same(r, b)
        };
        if structured {
            plan.marks.push(MarkMerge {
                path: path.clone(),
                base: b.cloned(),
                local: l.cloned(),
                remote: r.cloned(),
            });
            if let Some(entry) = l.or(r).or(b) {
                plan.merged.insert(path.clone(), entry.clone());
            }
        } else if same(l, r) {
            if let Some(entry) = l {
                plan.merged.insert(path.clone(), entry.clone());
            }
        } else if same(r, b) {
            // Local-only change (edit or delete) wins; deletion = omit.
            if let Some(entry) = l {
                plan.merged.insert(path.clone(), entry.clone());
            }
        } else if same(l, b) {
            // Remote-only change wins.
            match r {
                Some(entry) => {
                    plan.downloads.push((path.clone(), entry.clone()));
                    plan.merged.insert(path.clone(), entry.clone());
                }
                None => plan.delete_local.push(path.clone()),
            }
        } else {
            match (l, r) {
                // Divergent edits.
                (Some(le), Some(re)) => {
                    let remote_newer = re.mtime_ms >= le.mtime_ms;
                    let (cloud_wins, preserve) = match handling {
                        ConflictHandling::Review => {
                            plan.conflicts.push(SyncFileConflict::new(path, le, re));
                            continue;
                        }
                        ConflictHandling::Resolve(choices) => {
                            let cloud_wins = match choices.for_path(path) {
                                ConflictResolution::KeepCloud => true,
                                ConflictResolution::KeepLocal => false,
                                ConflictResolution::KeepBoth => remote_newer,
                            };
                            (cloud_wins, true)
                        }
                        ConflictHandling::Auto => (
                            remote_newer,
                            path.ends_with(".md")
                                || path.to_ascii_lowercase().ends_with(".table.json"),
                        ),
                    };
                    if preserve {
                        let loser = if cloud_wins { le } else { re };
                        let copy = (1..)
                            .map(|index| conflict_name(path, &loser.hash, index))
                            .find(|name| {
                                ![base, local, remote, &plan.merged]
                                    .iter()
                                    .any(|files| occupies_path(files, name))
                            })
                            .expect("unbounded conflict names");
                        plan.conflict_copies.push(copy.clone());
                        if cloud_wins {
                            plan.preserve_local.push((path.clone(), copy.clone()));
                        } else {
                            plan.downloads.push((copy.clone(), re.clone()));
                        }
                        plan.merged.insert(copy, loser.clone());
                    }
                    if cloud_wins {
                        plan.downloads.push((path.clone(), re.clone()));
                        plan.merged.insert(path.clone(), re.clone());
                    } else {
                        plan.merged.insert(path.clone(), le.clone());
                    }
                }
                // Delete vs modify → keep the modification.
                (Some(le), None) => {
                    plan.merged.insert(path.clone(), le.clone());
                }
                (None, Some(re)) => {
                    plan.downloads.push((path.clone(), re.clone()));
                    plan.merged.insert(path.clone(), re.clone());
                }
                (None, None) => {}
            }
        }
    }
    plan
}

/// Whether two manifests carry identical content, ignoring mtime: a download
/// rewrites the file with a fresh local mtime, so mtime drift alone must never
/// read as a change worth publishing.
fn same_content(a: &BTreeMap<String, FileEntry>, b: &BTreeMap<String, FileEntry>) -> bool {
    a.len() == b.len()
        && a.iter()
            .all(|(path, entry)| b.get(path).is_some_and(|o| o.hash == entry.hash))
}

/// Resolve structured candidates without mutating local files. A missing base
/// blob is not an empty base: fall back to preserving both exact versions.
async fn resolve_marks<S: RemoteStore>(
    vault: &Path,
    client: &S,
    plan: &mut MergePlan,
) -> Result<(), AppError> {
    for candidate in std::mem::take(&mut plan.marks) {
        let MarkMerge {
            path,
            base,
            local,
            remote,
        } = candidate;
        let local_bytes = match &local {
            Some(entry) => {
                let bytes = fs::read(vault.join(&path))?;
                if snapshot::hash_bytes(&bytes) != entry.hash {
                    return Err(changed_during_sync(&path));
                }
                Some(bytes)
            }
            None => None,
        };
        let remote_bytes = match &remote {
            Some(entry) => Some(
                fetch_blob(client, &path, entry)
                    .await?
                    .ok_or_else(|| AppError::message(format!("blob missing for {path}")))?,
            ),
            None => None,
        };
        let base_bytes = match &base {
            Some(entry) if local.as_ref().is_some_and(|l| l.hash == entry.hash) => {
                local_bytes.clone()
            }
            Some(entry) if remote.as_ref().is_some_and(|r| r.hash == entry.hash) => {
                remote_bytes.clone()
            }
            Some(entry) => fetch_blob(client, &path, entry).await?,
            None => None,
        };
        let mut versions: Vec<_> = local
            .iter()
            .zip(local_bytes.iter())
            .chain(remote.iter().zip(remote_bytes.iter()))
            .collect();
        if versions.is_empty() {
            versions.extend(base.iter().zip(base_bytes.iter()));
        }
        let result = if base.is_some() && base_bytes.is_none() {
            Err("base blob missing".into())
        } else {
            match mark_kind(&path) {
                Some(MarkKind::Annotations) => match (&local_bytes, &remote_bytes) {
                    (Some(l), Some(r)) => {
                        crate::features::pdf::annotation_merge::merge(base_bytes.as_deref(), l, r)
                            .map(|merged| (merged.bytes, merged.conflicted))
                    }
                    _ => Err("annotation source missing".into()),
                },
                Some(MarkKind::Ink(page)) => merge_ink(
                    page,
                    base_bytes.as_deref(),
                    local_bytes.as_deref(),
                    remote_bytes.as_deref(),
                ),
                _ => Err("unrecognized marks format".into()),
            }
        };
        let (bytes, conflicted) = match result {
            Ok(result) => result,
            Err(reason) => {
                log::warn!(target: "agentero::sync", "preserving marks conflict {path}: {reason}");
                // Stable winner independent of clocks and sync direction;
                // both sources are retained below, byte for byte.
                let (_, winner) = versions
                    .iter()
                    .max_by(|(a, _), (b, _)| a.hash.cmp(&b.hash))
                    .ok_or_else(|| {
                        AppError::message(format!("retained ink blob missing for {path}"))
                    })?;
                ((*winner).clone(), true)
            }
        };
        if conflicted {
            for &(entry, raw) in &versions {
                let copy = mark_conflict_name(&path, &entry.hash);
                if plan
                    .merged
                    .get(&copy)
                    .is_some_and(|old| old.hash != entry.hash)
                {
                    return Err(AppError::message(format!(
                        "marks conflict copy already differs: {copy}"
                    )));
                }
                match fs::read(vault.join(&copy)) {
                    Ok(existing) if snapshot::hash_bytes(&existing) != entry.hash => {
                        return Err(AppError::message(format!(
                            "marks conflict copy already differs: {copy}"
                        )));
                    }
                    Ok(_) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e.into()),
                }
                if !plan.merged.contains_key(&copy) {
                    plan.merged.insert(copy.clone(), entry.clone());
                    plan.writes.insert(copy.clone(), raw.clone());
                } else if durable_ink_path(&copy)
                    && !plan.downloads.iter().any(|(path, _)| path == &copy)
                {
                    // A previous post-rename failure may have left this exact
                    // copy visible but not durable. Confirm it again before a
                    // canonical replacement can discard its sole source.
                    plan.writes.insert(copy.clone(), raw.clone());
                }
                plan.delete_local.retain(|path| path != &copy);
                plan.mark_conflicts.push(copy);
            }
        }
        let hash = snapshot::hash_bytes(&bytes);
        let entry = FileEntry {
            mtime_ms: versions
                .iter()
                .find(|(entry, _)| entry.hash == hash)
                .map(|(entry, _)| entry.mtime_ms)
                .unwrap_or_else(|| {
                    versions
                        .iter()
                        .map(|(entry, _)| entry.mtime_ms)
                        .max()
                        .unwrap_or(0)
                }),
            size: bytes.len() as u64,
            hash,
        };
        if local.as_ref().is_none_or(|local| entry.hash != local.hash) {
            plan.writes.insert(path.clone(), bytes);
        }
        plan.merged.insert(path, entry);
    }
    Ok(())
}

fn merge_ink(
    page: u32,
    base: Option<&[u8]>,
    local: Option<&[u8]>,
    remote: Option<&[u8]>,
) -> Result<(Vec<u8>, bool), String> {
    use crate::features::pdf::annotation_merge::parse_json;
    use crate::features::pdf::ink::InkPage;
    // Strict object-key parsing plus the model's deny_unknown_fields avoids
    // silently normalizing unfamiliar/ambiguous data into a smaller document.
    let parse =
        |bytes: Option<&[u8]>| -> Result<(InkPage, BTreeMap<uuid::Uuid, serde_json::Value>, bool), String> {
            let Some(bytes) = bytes else {
                return Ok((InkPage::new(page).map_err(|e| e.to_string())?, BTreeMap::new(), false));
            };
            let value = parse_json(bytes).map_err(|e| e.to_string())?;
            let raw: InkPage = serde_json::from_value(value).map_err(|e| e.to_string())?;
            raw.validate().map_err(|e| e.to_string())?;
            if raw.page != page {
                return Err("ink page does not match filename".into());
            }
            let mut records = BTreeMap::new();
            let mut ambiguous = false;
            for (id, value) in raw
                .strokes
                .iter()
                .map(|s| (s.id, serde_json::to_value(s)))
                .chain(
                    raw.tombstones
                        .iter()
                        .map(|s| (s.id, serde_json::to_value(s))),
                )
            {
                let value = value.map_err(|e| e.to_string())?;
                if let Some(previous) = records.insert(id, value.clone()) {
                    ambiguous |= previous != value;
                }
            }
            let normalized = raw.merge(&raw).map_err(|e| e.to_string())?;
            Ok((normalized, records, ambiguous))
        };
    let (l, lr, la) = parse(local)?;
    let (r, rr, ra) = parse(remote)?;
    let (b, br, ba) = parse(base)?;
    let mut conflicted = la
        || ra
        || ba
        || lr.iter().any(|(id, l)| {
            rr.get(id)
                .is_some_and(|r| l != r && br.get(id) != Some(l) && br.get(id) != Some(r))
        });
    // Include the base because omission is never a stroke/tombstone deletion.
    let merged = b
        .merge(&l)
        .and_then(|m| m.merge(&r))
        .map_err(|e| e.to_string())?;
    let winners: BTreeMap<_, _> = merged
        .strokes
        .iter()
        .map(|s| (s.id, serde_json::to_value(s)))
        .chain(
            merged
                .tombstones
                .iter()
                .map(|s| (s.id, serde_json::to_value(s))),
        )
        .map(|(id, value)| value.map(|value| (id, value)))
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
    // A writer may reuse or decrease a revision on a one-sided edit. If the
    // model rejects that changed payload in favor of the base, retain it too.
    conflicted |= lr
        .iter()
        .chain(rr.iter())
        .any(|(id, value)| br.get(id) != Some(value) && winners.get(id) != Some(value));
    Ok((
        merged.to_json().map_err(|e| e.to_string())?.into_bytes(),
        conflicted,
    ))
}

/// Apply the merged plan locally under the file-mutation lane, BEFORE remote
/// publication (mobile durability order). Each download/write/delete rechecks
/// the scanned baseline inside the lane so a cooperating editor's concurrent
/// save is rejected ("changed during sync") rather than silently overwritten.
/// Ink replacements fsync before rename and flush their directories (see
/// `write_atomic`); a failed flush aborts the pass before any upload/HEAD.
async fn apply_local<S: RemoteStore>(
    vault: &Path,
    client: &S,
    plan: &MergePlan,
    scanned: &BTreeMap<String, FileEntry>,
    outcome: &mut SyncOutcome,
    progress: Progress<'_>,
) -> Result<(), AppError> {
    for (from, to) in &plan.preserve_local {
        crate::fs::with_file_mutation(|| {
            ensure_current(vault, from, scanned.get(from))?;
            ensure_current(vault, to, None)?;
            let target = vault.join(to);
            let parent = target
                .parent()
                .expect("conflict copy is a vault descendant");
            let staged = tempfile::Builder::new()
                .suffix(".tmp")
                .tempfile_in(parent)?;
            fs::copy(vault.join(from), staged.path())?;
            if snapshot::hash_file(staged.path())? != plan.merged[to].hash {
                return Err(changed_during_sync(from));
            }
            staged.as_file().sync_all()?;
            staged
                .persist_noclobber(&target)
                .map_err(|error| error.error)?;
            #[cfg(unix)]
            fs::File::open(parent)?.sync_all()?;
            Ok(())
        })?;
    }
    let total = plan.downloads.len();
    for (i, (rel, entry)) in plan.downloads.iter().enumerate() {
        progress("download", i + 1, total);
        let raw = fetch_blob(client, rel, entry)
            .await?
            .ok_or_else(|| AppError::message(format!("blob missing for {rel}")))?;
        crate::fs::with_file_mutation(|| {
            ensure_current(vault, rel, scanned.get(rel))?;
            write_atomic(vault, rel, &raw)
        })?;
        outcome.downloaded += 1;
    }
    for (i, (rel, raw)) in plan.writes.iter().enumerate() {
        progress("download", i + 1, plan.writes.len());
        crate::fs::with_file_mutation(|| {
            ensure_current(vault, rel, scanned.get(rel))?;
            write_atomic(vault, rel, raw)
        })?;
        outcome.downloaded += 1;
    }
    for copy in plan.conflict_copies.iter().chain(&plan.mark_conflicts) {
        if !outcome.conflict_copies.contains(copy) {
            outcome.conflict_copies.push(copy.clone());
        }
    }
    for rel in &plan.delete_local {
        let removed = crate::fs::with_file_mutation(|| {
            ensure_current(vault, rel, scanned.get(rel))?;
            match fs::remove_file(vault.join(rel)) {
                Ok(()) => Ok(true),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
                Err(e) => Err(AppError::message(format!("delete {rel}: {e}"))),
            }
        })?;
        if removed {
            outcome.deleted_local += 1;
        }
    }
    Ok(())
}

async fn fetch_blob<S: RemoteStore>(
    client: &S,
    path: &str,
    entry: &FileEntry,
) -> Result<Option<Vec<u8>>, AppError> {
    let Some((bytes, _)) = client.get(&blob_key(&entry.hash)).await? else {
        return Ok(None);
    };
    let raw = gunzip_limited(&bytes, entry.size.saturating_add(1 << 20))?;
    if snapshot::hash_bytes(&raw) != entry.hash {
        return Err(AppError::message(format!("blob corrupt for {path}")));
    }
    Ok(Some(raw))
}

fn mark_conflict_name(path: &str, hash: &str) -> String {
    match path.rsplit_once('.') {
        Some((stem, ext)) => format!("{stem} (conflict {hash}).{ext}"),
        None => format!("{path} (conflict {hash})"),
    }
}

fn changed_during_sync(path: &str) -> AppError {
    AppError::message(format!("{path} changed during sync; run sync again"))
}

/// Check immediately before overwrite, after any awaited blob requests and
/// progress callbacks. The vault sync lock does not lock an active editor.
fn ensure_current(vault: &Path, path: &str, expected: Option<&FileEntry>) -> Result<(), AppError> {
    match (snapshot::hash_file(&vault.join(path)), expected) {
        (Ok(hash), Some(entry)) if hash == entry.hash => Ok(()),
        (Err(_), None) if !vault.join(path).try_exists()? => Ok(()),
        _ => Err(changed_during_sync(path)),
    }
}

/// Downloaded sidecars / deleted paper folders must be reflected in catalog.
pub(super) fn refresh_catalog_if_needed(vault: &Path) -> Result<(), AppError> {
    let marker = local::sync_dir(vault).join("catalog.dirty");
    if marker.try_exists()? {
        crate::features::paper::catalog::papers::rebuild_from_disk(vault)?;
        crate::features::paper::catalog::papers::prune_missing(vault)?;
        fs::remove_file(marker)?;
    }
    Ok(())
}

fn blob_key(hash: &str) -> String {
    format!("blobs/{}/{hash}", &hash[..2])
}

fn read_blob(vault: &Path, rel: &str, entry: &FileEntry) -> Result<Vec<u8>, AppError> {
    let raw = fs::read(vault.join(rel))?;
    if snapshot::hash_bytes(&raw) != entry.hash {
        return Err(AppError::message(format!(
            "{rel} changed during sync; run sync again"
        )));
    }
    gzip(&raw)
}

fn occupies_path(files: &BTreeMap<String, FileEntry>, path: &str) -> bool {
    let children = format!("{path}/");
    files.contains_key(path)
        || files
            .range(children.clone()..)
            .next()
            .is_some_and(|(name, _)| name.starts_with(&children))
}

pub(super) fn conflict_name(path: &str, hash: &str, index: u64) -> String {
    let stamp = &hash[..hash.len().min(12)];
    let stamp = if index == 1 {
        stamp.to_string()
    } else {
        format!("{stamp}-{index}")
    };
    if path.to_ascii_lowercase().ends_with(".table.json") {
        let stem = &path[..path.len() - ".table.json".len()];
        return format!("{stem} (conflict {stamp}).table.json");
    }
    match path.rsplit_once('.') {
        Some((stem, ext)) if !ext.contains('/') => format!("{stem} (conflict {stamp}).{ext}"),
        _ => format!("{path} (conflict {stamp})"),
    }
}

fn write_atomic(vault: &Path, rel: &str, bytes: &[u8]) -> Result<(), AppError> {
    #[cfg(test)]
    {
        tests::write_atomic_for_test(vault, rel, bytes)
    }
    #[cfg(not(test))]
    {
        publish_sync_file(vault, rel, bytes, crate::fs::atomic_write_with, |path| {
            flush_ink_directories(path, |dir| fs::File::open(dir)?.sync_all())
        })
    }
}

fn durable_ink_path(rel: &str) -> bool {
    if matches!(mark_kind(rel), Some(MarkKind::Ink(_))) {
        return true;
    }
    let Some(mut stem) = rel.strip_suffix(".json") else {
        return false;
    };
    loop {
        // A losing acknowledged stroke may survive only in a generated raw
        // conflict sibling. Also retain durability if such a sibling conflicts.
        let Some((original, suffix)) = stem.rsplit_once(" (conflict ") else {
            return false;
        };
        let Some(hash) = suffix.strip_suffix(')') else {
            return false;
        };
        if hash.len() != 64 || !hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return false;
        }
        if matches!(
            mark_kind(&format!("{original}.json")),
            Some(MarkKind::Ink(_))
        ) {
            return true;
        }
        stem = original;
    }
}

fn flush_ink_directories(
    path: &Path,
    mut flush: impl FnMut(&Path) -> std::io::Result<()>,
) -> std::io::Result<()> {
    // Bound publication to ink/, marks/, and the existing paper directory,
    // matching native ink saves. Never walk through arbitrary vault ancestors.
    for directory in path
        .ancestors()
        .skip(1)
        .take(if cfg!(unix) { 3 } else { 0 })
    {
        flush(directory)?;
    }
    Ok(())
}

fn publish_sync_file(
    vault: &Path,
    rel: &str,
    bytes: &[u8],
    publish: impl FnOnce(&Path, &[u8], &crate::fs::AtomicOpts) -> std::io::Result<()>,
    flush_directories: impl FnOnce(&Path) -> std::io::Result<()>,
) -> Result<(), AppError> {
    let target = vault.join(rel);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    let ink = durable_ink_path(rel);
    // Keep unique sibling *.tmp staging and all caller mutation-lane checks.
    // Ink must reach stable storage BEFORE replacing the acknowledged version.
    publish(
        &target,
        bytes,
        &crate::fs::AtomicOpts {
            sync_file: ink,
            ..Default::default()
        },
    )?;
    if ink {
        // Rename already succeeded. The new bytes may be visible; do not roll
        // back or continue to uploads/HEAD publication after a failed flush.
        flush_directories(&target).map_err(|_| AppError::domain(
            "sync_ink_durability_uncertain",
            "Ink sync replacement may already be visible; reload the page to reconcile before retrying",
        ))?;
    }
    Ok(())
}

fn ensure_materialized_snapshot(vault: &Path, scope: &SyncScope) -> Result<(), AppError> {
    let state = availability::read_state(vault)?;
    if state
        .deferred
        .iter()
        .any(|path| !snapshot::is_scope_excluded(scope, path))
    {
        return Err(AppError::domain(
            "sync_snapshot_incomplete",
            "Download deferred vault assets before publishing a complete snapshot",
        ));
    }
    Ok(())
}

// ---- Read-only mirrors -----------------------------------------------------
//
// A mirror is an output target, never an input: a pass publishes the vault's
// local state and never merges the mirror's manifest back. Two bidirectional
// backends would each carry their own merge base and fight over deletions, so
// the authoritative side stays singular by construction.

/// Test a mirror binding. Unlike [`test_connection`], the primary's rules do
/// not apply: a Git mirror branch is only a push target (the vault's own branch
/// and `origin` stay untouched), and object-store mirrors just need a usable
/// root with conditional writes for the HEAD CAS.
pub async fn test_mirror_connection(
    vault: &Path,
    cfg: &SyncBackendConfig,
) -> Result<bool, AppError> {
    crate::features::sync::ensure_vault_sync_available(vault)?;
    ensure_mirror_config(cfg)?;
    if cfg.backend == SyncBackendKind::Git {
        crate::features::sync::git::test_mirror_connection(vault, cfg).await?;
        return Ok(true);
    }
    let client = SyncStore::new(cfg, vault)?;
    client.ensure_root().await?;
    if !client.probe_conditional_writes().await? {
        return Err(AppError::domain(
            super::SYNC_NO_CONDITIONAL_WRITES,
            "This mirror store does not enforce conditional writes",
        ));
    }
    Ok(true)
}

fn ensure_mirror_config(cfg: &SyncBackendConfig) -> Result<(), AppError> {
    if cfg.backend == SyncBackendKind::Agentero {
        return Err(AppError::message(
            "Agentero cloud cannot be a mirror backend",
        ));
    }
    cfg.validate()
}

/// One pass that publishes `cfg` as a read-only mirror of `vault`.
///
/// The remote layout matches the primary's, so another Agentero can still open
/// the mirror as a normal store; this device just never reads it back.
pub async fn publish_mirror(
    vault: &Path,
    cfg: &SyncBackendConfig,
    progress: Progress<'_>,
) -> Result<SyncOutcome, AppError> {
    crate::features::sync::ensure_vault_sync_available(vault)?;
    ensure_mirror_config(cfg)?;
    if conflicts::read_review(vault)?.is_some() || recovery::unfinished(vault)? {
        return Err(AppError::domain(
            "sync_primary_unsettled",
            "Finish the primary sync before publishing this mirror",
        ));
    }
    ensure_materialized_snapshot(vault, &cfg.scope)?;
    if cfg.backend == SyncBackendKind::Git {
        return crate::features::sync::git::publish_mirror(vault, cfg, progress).await;
    }
    let client = SyncStore::new(cfg, vault)?;
    client.ensure_root().await?;
    if !client.probe_conditional_writes().await? {
        return Err(AppError::domain(
            super::SYNC_NO_CONDITIONAL_WRITES,
            "This mirror store does not enforce conditional writes",
        ));
    }
    run_mirror(vault, cfg, &client, progress).await
}

async fn run_mirror<S: RemoteStore>(
    vault: &Path,
    cfg: &SyncBackendConfig,
    client: &S,
    progress: Progress<'_>,
) -> Result<SyncOutcome, AppError> {
    let store_id = config::backend_identity(cfg);
    let state_dir = local::mirror_dir(vault, &store_id);
    progress("scan", 0, 0);
    let base = local::read_base_at(&state_dir)?;
    // The base doubles as the scan cache: unchanged files keep their mtime/hash.
    let files = visible_files(
        scan_local(vault, base.clone(), cfg.scope).await?,
        &cfg.scope,
    );
    validate_tree(&files)?;
    let mut outcome = SyncOutcome::default();
    ensure_mirror_identity(vault, client).await?;

    for _ in 0..MAX_CAS_RETRIES {
        progress("pull", 0, 0);
        let (head_ptr, head_etag) = match client.get(HEAD_KEY).await? {
            Some((bytes, etag)) => (
                Some(serde_json::from_slice::<HeadPointer>(&bytes)?),
                Some(etag),
            ),
            None => (None, None),
        };
        let remote = match &head_ptr {
            Some(ptr) => load_manifest(client, ptr).await?,
            None => Manifest::default(),
        };
        let mut files = files.clone();
        files.extend(
            remote
                .files
                .iter()
                .filter(|(path, _)| snapshot::is_scope_excluded(&cfg.scope, path))
                .map(|(path, entry)| (path.clone(), entry.clone())),
        );
        validate_tree(&files)?;
        if let Some(head) = head_ptr
            .as_ref()
            .filter(|_| same_content(&files, &remote.files))
        {
            record_mirror_snapshot(&state_dir, &store_id, head, &remote, &cfg.scope)?;
            outcome.version = head.version;
            return Ok(outcome);
        }
        let known: HashSet<&str> = remote
            .files
            .values()
            .map(|entry| entry.hash.as_str())
            .collect();
        let mut uploaded = 0;
        let total = files.len();
        for (i, (rel, entry)) in files.iter().enumerate() {
            if known.contains(entry.hash.as_str()) {
                continue;
            }
            progress("upload", i + 1, total);
            match client
                .put(
                    &blob_key(&entry.hash),
                    read_blob(vault, rel, entry)?,
                    PutCondition::IfNoneMatch,
                )
                .await?
            {
                PutOutcome::Ok => uploaded += 1,
                // Already present (a previous pass uploaded it before publishing).
                PutOutcome::PreconditionFailed => {}
            }
        }
        outcome.uploaded += uploaded;

        progress("finalize", 0, 0);
        let new_version = head_ptr
            .as_ref()
            .map_or(0, |pointer| pointer.version)
            .checked_add(1)
            .ok_or_else(|| AppError::message("sync version exhausted"))?;
        let manifest_key = format!(
            "manifests/{new_version:010}-{}.json.gz",
            &uuid::Uuid::new_v4().simple().to_string()[..8]
        );
        // Keep the scope the mirror was first published with; changing the
        // primary's scope must not silently reinterpret an existing mirror.
        let scope = head_ptr.as_ref().map_or(cfg.scope, |_| remote.scope);
        let manifest = Manifest {
            version: new_version,
            files: files.clone(),
            scope,
        };
        validate_manifest(&manifest)?;
        if client
            .put(
                &manifest_key,
                gzip(&serde_json::to_vec(&manifest)?)?,
                PutCondition::IfNoneMatch,
            )
            .await?
            != PutOutcome::Ok
        {
            continue;
        }
        let next_head = HeadPointer {
            version: new_version,
            manifest_key,
            updated_at: now(),
        };
        let condition = match head_etag {
            Some(etag) => PutCondition::IfMatch(etag),
            None => PutCondition::IfNoneMatch,
        };
        if client
            .put(HEAD_KEY, serde_json::to_vec(&next_head)?, condition)
            .await?
            == PutOutcome::PreconditionFailed
        {
            continue;
        }
        record_mirror_snapshot(&state_dir, &store_id, &next_head, &manifest, &cfg.scope)?;
        outcome.removed_remote = remote
            .files
            .keys()
            .filter(|path| !files.contains_key(*path))
            .count();
        outcome.version = next_head.version;
        return Ok(outcome);
    }
    Err(AppError::message(
        "mirror contention: too many concurrent updates, try again",
    ))
}

fn record_mirror_snapshot(
    dir: &Path,
    store_id: &str,
    head: &HeadPointer,
    manifest: &Manifest,
    local_scope: &SyncScope,
) -> Result<(), AppError> {
    local::write_base_at(
        dir,
        &Manifest {
            version: head.version,
            files: visible_files(manifest.files.clone(), local_scope),
            scope: *local_scope,
        },
    )?;
    local::write_meta_at(
        dir,
        &SyncMeta {
            last_sync_at: Some(now()),
            last_version: head.version,
            store_id: Some(store_id.to_owned()),
            manifest_key: Some(head.manifest_key.clone()),
            ..SyncMeta::default()
        },
    )
}

/// Bind a mirror store to this vault. A mirror that already holds another
/// vault's id is refused instead of being overwritten: pointing two vaults at
/// one mirror would make the second pass silently erase the first one.
async fn ensure_mirror_identity<S: RemoteStore>(vault: &Path, client: &S) -> Result<(), AppError> {
    let vault_id = local::ensure_vault_id(vault)?;
    let (bytes, _) = match client.get(VAULT_KEY).await? {
        Some(record) => record,
        None => {
            client
                .put(
                    VAULT_KEY,
                    serde_json::to_vec(&RemoteVaultInfo {
                        vault_id: vault_id.clone(),
                        format_version: FORMAT_VERSION,
                        encryption: "none".into(),
                    })?,
                    PutCondition::IfNoneMatch,
                )
                .await?;
            client.get(VAULT_KEY).await?.ok_or_else(|| {
                AppError::message("mirror store has no vault identity after initialization")
            })?
        }
    };
    let info: RemoteVaultInfo = serde_json::from_slice(&bytes)
        .map_err(|_| AppError::message("mirror store is not an Agentero sync store"))?;
    if info.format_version > FORMAT_VERSION {
        return Err(AppError::message(
            "mirror sync store uses a newer format; update the app",
        ));
    }
    if info.vault_id != vault_id {
        return Err(AppError::message(
            "mirror store belongs to a different vault; pick an empty prefix",
        ));
    }
    Ok(())
}

/// Per-mirror pass state. Progress phases stay vault-level (both passes share
/// them), so the host gets one event per mirror instead of a phase stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MirrorPhase {
    Started,
    Published,
    Failed,
}

/// Called once per mirror pass: `(index, mirror, phase, error)`.
pub type MirrorWatch<'a> =
    &'a (dyn Fn(usize, &SyncBackendConfig, MirrorPhase, Option<&str>) + Send + Sync);

/// One pass under the vault lock: the authoritative primary, then every
/// configured mirror in order.
///
/// A mirror failure never fails the pass — the primary already succeeded and
/// the mirror records its own `lastError` for the next attempt. A primary
/// failure stops before the mirrors, because publishing a half-applied local
/// tree would only spread it.
pub async fn sync_all_locked(
    lock: &super::lock::SyncLock,
    cfg: &config::VaultSyncConfig,
    progress: Progress<'_>,
    watch: Option<MirrorWatch<'_>>,
) -> Result<SyncOutcome, AppError> {
    let outcome = sync_vault_locked(lock, &cfg.primary, progress).await?;
    publish_mirrors_locked(lock, cfg, progress, watch).await;
    Ok(outcome)
}

/// Publish mirrors after a successful primary pass or conflict resolution.
pub async fn publish_mirrors_locked(
    lock: &super::lock::SyncLock,
    cfg: &config::VaultSyncConfig,
    progress: Progress<'_>,
    watch: Option<MirrorWatch<'_>>,
) {
    let vault = lock.vault();
    for (index, mirror) in cfg.mirrors.iter().enumerate() {
        // Scope is a whole-vault policy: a mirror always publishes exactly what
        // the primary does, so a stale mirror binding cannot shrink the copy.
        let mut mirror_cfg = mirror.clone();
        mirror_cfg.scope = cfg.primary.scope;
        if let Some(watch) = watch {
            watch(index, &mirror_cfg, MirrorPhase::Started, None);
        }
        let result = if cfg.duplicate_remote().is_some() {
            Err(AppError::message(
                "sync backends must use distinct destinations",
            ))
        } else {
            publish_mirror(vault, &mirror_cfg, progress).await
        };
        if let Err(error) = result {
            let state_dir = local::mirror_dir(vault, &config::backend_identity(&mirror_cfg));
            let message = match error.code() {
                "sync_snapshot_incomplete" => {
                    "Download deferred vault assets to publish this mirror"
                }
                "sync_primary_unsettled" => "Finish the primary sync before publishing this mirror",
                super::SYNC_NO_CONDITIONAL_WRITES => {
                    "This mirror store does not enforce conditional writes"
                }
                _ => "Mirror publication failed. Check its connection settings and try again",
            };
            local::record_error_at(&state_dir, message);
            log::warn!(
                target: "agentero::sync",
                "mirror pass failed backend={:?}: {message}",
                mirror.backend
            );
            if let Some(watch) = watch {
                watch(index, &mirror_cfg, MirrorPhase::Failed, Some(message));
            }
        } else if let Some(watch) = watch {
            watch(index, &mirror_cfg, MirrorPhase::Published, None);
        }
    }
}

/// Last pass result for one mirror (settings pane / CLI status).
#[derive(Debug, Default, Clone)]
pub struct MirrorState {
    pub last_sync_at: Option<String>,
    pub last_version: u64,
    pub last_error: Option<String>,
}

pub fn mirror_state(vault: &Path, cfg: &SyncBackendConfig) -> MirrorState {
    let dir = local::mirror_dir(vault, &config::backend_identity(cfg));
    let meta = local::read_meta_at(&dir).unwrap_or_default();
    MirrorState {
        last_sync_at: meta.last_sync_at,
        last_version: meta.last_version,
        last_error: meta.last_error,
    }
}

fn gzip(raw: &[u8]) -> Result<Vec<u8>, AppError> {
    let mut enc = GzEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(raw)?;
    Ok(enc.finish()?)
}

/// Decompress with an output cap; reading `limit + 1` bytes detects overflow
/// without allocating it.
fn gunzip_limited(bytes: &[u8], limit: u64) -> Result<Vec<u8>, AppError> {
    let mut out = Vec::new();
    let n = GzDecoder::new(bytes)
        .take(limit.saturating_add(1))
        .read_to_end(&mut out)?;
    if n as u64 > limit {
        return Err(AppError::message(format!(
            "decompressed data exceeds the {limit} byte limit"
        )));
    }
    Ok(out)
}

fn now() -> String {
    crate::time::now_rfc3339_millis()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    fn entry(hash: &str, mtime: i64) -> FileEntry {
        FileEntry {
            hash: hash.into(),
            size: 1,
            mtime_ms: mtime,
        }
    }

    fn map(pairs: &[(&str, FileEntry)]) -> BTreeMap<String, FileEntry> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    /// In-memory `RemoteStore` that counts requests, so a test can assert what
    /// a pass costs and not only what it produces.
    struct MemStore {
        objects: Mutex<BTreeMap<String, (Vec<u8>, String)>>,
        revisions: AtomicUsize,
        gets: AtomicUsize,
        puts: AtomicUsize,
        /// PUTs allowed before the store starts refusing — models a rate limit
        /// landing partway through a pass.
        put_budget: AtomicUsize,
        degraded: bool,
    }

    impl Default for MemStore {
        fn default() -> Self {
            Self {
                objects: Mutex::new(BTreeMap::new()),
                revisions: AtomicUsize::new(0),
                gets: AtomicUsize::new(0),
                puts: AtomicUsize::new(0),
                put_budget: AtomicUsize::new(usize::MAX),
                degraded: false,
            }
        }
    }

    impl MemStore {
        fn counts(&self) -> (usize, usize) {
            (
                self.gets.load(Ordering::Relaxed),
                self.puts.load(Ordering::Relaxed),
            )
        }
    }

    impl RemoteStore for MemStore {
        async fn ensure_root(&self) -> Result<(), AppError> {
            Ok(())
        }

        async fn get(&self, key: &str) -> Result<Option<(Vec<u8>, String)>, AppError> {
            self.gets.fetch_add(1, Ordering::Relaxed);
            Ok(self.objects.lock().unwrap().get(key).cloned())
        }

        async fn put(
            &self,
            key: &str,
            body: Vec<u8>,
            condition: PutCondition,
        ) -> Result<PutOutcome, AppError> {
            let attempt = self.puts.fetch_add(1, Ordering::Relaxed) + 1;
            if attempt > self.put_budget.load(Ordering::Relaxed) {
                return Err(AppError::message("PUT {key}: 429 Too Many Requests"));
            }
            let mut objects = self.objects.lock().unwrap();
            let current = objects.get(key).map(|(_, etag)| etag.clone());
            if self.degraded
                && key == HEAD_KEY
                && !matches!(&condition, PutCondition::Unconditional)
            {
                panic!("degraded HEAD publication requires an unconditional PUT");
            }
            match (&current, condition) {
                (Some(_), PutCondition::IfNoneMatch) => return Ok(PutOutcome::PreconditionFailed),
                (None, PutCondition::IfMatch(_)) => return Ok(PutOutcome::PreconditionFailed),
                (Some(etag), PutCondition::IfMatch(want)) if *etag != want => {
                    return Ok(PutOutcome::PreconditionFailed)
                }
                _ => {}
            }
            let etag = format!("\"r{}\"", self.revisions.fetch_add(1, Ordering::Relaxed));
            objects.insert(key.to_string(), (body, etag));
            Ok(PutOutcome::Ok)
        }

        async fn probe_conditional_writes(&self) -> Result<bool, AppError> {
            Ok(!self.degraded)
        }

        async fn try_acquire_publish_lock(
            &self,
            _device_id: &str,
        ) -> Result<super::super::store::LockOutcome, AppError> {
            Ok(if self.degraded {
                super::super::store::LockOutcome::Acquired
            } else {
                super::super::store::LockOutcome::Unsupported
            })
        }
    }

    fn temp_vault(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("agentero-sync-{tag}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn noop_progress() -> &'static (dyn Fn(&str, usize, usize) + Send + Sync) {
        &|_, _, _| {}
    }

    #[tokio::test]
    async fn degraded_webdav_preserves_review_and_conflict_copy() {
        let store = MemStore {
            degraded: true,
            ..MemStore::default()
        };
        let cfg = SyncBackendConfig {
            backend: SyncBackendKind::Webdav,
            allow_degraded_sync: true,
            conflict_policy: ConflictPolicy::Ask,
            ..SyncBackendConfig::default()
        };
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let rel = "papers/x/NOTES.md";
        fs::create_dir_all(a.path().join("papers/x")).unwrap();
        fs::write(a.path().join(rel), "base").unwrap();
        run_sync(a.path(), &cfg, &store, noop_progress())
            .await
            .unwrap();
        run_sync(b.path(), &cfg, &store, noop_progress())
            .await
            .unwrap();
        // Use a different size as well as bytes to invalidate the scan cache.
        fs::write(a.path().join(rel), "cloud").unwrap();
        fs::write(b.path().join(rel), "local version").unwrap();
        run_sync(a.path(), &cfg, &store, noop_progress())
            .await
            .unwrap();
        let puts = store.counts().1;
        let error = run_sync(b.path(), &cfg, &store, noop_progress())
            .await
            .unwrap_err();
        assert_eq!(error.code(), conflicts::PENDING_CODE);
        assert_eq!(store.counts().1, puts);
        assert_eq!(
            fs::read_to_string(b.path().join(rel)).unwrap(),
            "local version"
        );
        let review = conflicts::read_pending(b.path()).unwrap().unwrap();
        let lock = super::super::lock::SyncLock::acquire(b.path()).unwrap();
        conflicts::accept(
            &lock,
            &review.review_id,
            ConflictResolution::KeepLocal,
            BTreeMap::new(),
        )
        .unwrap();
        let resolved = run_sync(b.path(), &cfg, &store, noop_progress())
            .await
            .unwrap();
        assert_eq!(resolved.version, 3);
        assert_eq!(
            fs::read_to_string(b.path().join(rel)).unwrap(),
            "local version"
        );
        assert_eq!(resolved.conflict_copies.len(), 1);
        assert_eq!(
            fs::read_to_string(b.path().join(&resolved.conflict_copies[0])).unwrap(),
            "cloud"
        );
        assert!(conflicts::read_pending(b.path()).unwrap().is_none());
    }

    #[tokio::test]
    async fn mirror_rejects_deferred_assets_before_connecting_to_store() {
        let vault = tempfile::tempdir().unwrap();
        let mut availability = AvailabilityState::default();
        availability.deferred.insert("papers/x/paper.pdf".into());
        availability::write_state(vault.path(), &availability).unwrap();
        let cfg = SyncBackendConfig {
            backend: SyncBackendKind::Webdav,
            webdav_url: "https://example.invalid/dav".into(),
            webdav_username: "user".into(),
            webdav_password: "password".into(),
            ..SyncBackendConfig::default()
        };
        let error = publish_mirror(vault.path(), &cfg, noop_progress())
            .await
            .unwrap_err();
        assert_eq!(error.code(), "sync_snapshot_incomplete");
        assert_eq!(mirror_state(vault.path(), &cfg).last_version, 0);
    }

    /// The point of the up-to-date check: an idle vault costs one request per
    /// pass instead of a full pull/merge/publish round trip.
    #[tokio::test]
    async fn idle_pass_costs_one_request() {
        let store = MemStore::default();
        let cfg = SyncBackendConfig::default();
        let vault = temp_vault("idle");
        fs::create_dir_all(vault.join("papers/x")).unwrap();
        fs::write(vault.join("papers/x/NOTES.md"), "# x\n").unwrap();

        let first = run_sync(&vault, &cfg, &store, noop_progress())
            .await
            .unwrap();
        assert_eq!((first.version, first.uploaded), (1, 1));

        let before = store.counts();
        let second = run_sync(&vault, &cfg, &store, noop_progress())
            .await
            .unwrap();
        assert_eq!(second.version, 1);
        let (gets, puts) = store.counts();
        assert_eq!((gets - before.0, puts - before.1), (1, 0));

        let _ = fs::remove_dir_all(&vault);
    }

    #[tokio::test]
    async fn projection_failure_stops_sync_before_remote_io() {
        use crate::features::paper::catalog::papers::{self, PaperRecord};
        let vault = tempfile::tempdir().unwrap();
        fs::create_dir_all(vault.path().join("papers/x/metadata.json")).unwrap();
        papers::upsert_paper(
            vault.path(),
            &PaperRecord::local_pdf("x".into(), "X".into()).at_path("papers/x"),
        )
        .unwrap();
        let store = MemStore::default();
        assert!(run_sync(
            vault.path(),
            &SyncBackendConfig::default(),
            &store,
            noop_progress()
        )
        .await
        .is_err());
        assert_eq!(store.counts(), (0, 0));
    }

    /// Two idle devices must not advance HEAD. A download rewrites the file
    /// with a fresh local mtime, and republishing that difference used to make
    /// each device outbid the other forever.
    #[tokio::test]
    async fn idle_devices_do_not_ping_pong_head() {
        let store = MemStore::default();
        let cfg = SyncBackendConfig::default();
        let (a, b) = (temp_vault("ping-a"), temp_vault("ping-b"));
        fs::create_dir_all(a.join("papers/x")).unwrap();
        fs::create_dir_all(&b).unwrap();
        fs::write(a.join("papers/x/NOTES.md"), "# x\n").unwrap();

        let published = run_sync(&a, &cfg, &store, noop_progress()).await.unwrap();
        assert_eq!(published.version, 1);
        // B joins by download only: it adopts the remote manifest instead of
        // publishing a content-identical one.
        let joined = run_sync(&b, &cfg, &store, noop_progress()).await.unwrap();
        assert_eq!((joined.version, joined.downloaded), (1, 1));

        for round in 0..2 {
            let before = store.counts();
            let again_a = run_sync(&a, &cfg, &store, noop_progress()).await.unwrap();
            let again_b = run_sync(&b, &cfg, &store, noop_progress()).await.unwrap();
            assert_eq!((again_a.version, again_b.version), (1, 1), "round {round}");
            let (gets, puts) = store.counts();
            assert_eq!(puts - before.1, 0, "round {round} republished");
            assert_eq!(gets - before.0, 2, "round {round}: one HEAD read each");
        }

        // A real edit still publishes, and B still picks it up.
        fs::write(a.join("papers/x/NOTES.md"), "# x\nedited\n").unwrap();
        let edited = run_sync(&a, &cfg, &store, noop_progress()).await.unwrap();
        assert_eq!((edited.version, edited.uploaded), (2, 1));
        let pulled = run_sync(&b, &cfg, &store, noop_progress()).await.unwrap();
        assert_eq!((pulled.version, pulled.downloaded), (2, 1));
        assert!(fs::read_to_string(b.join("papers/x/NOTES.md"))
            .unwrap()
            .contains("edited"));

        for dir in [a, b] {
            let _ = fs::remove_dir_all(dir);
        }
    }

    /// An interrupted upload must not be re-sent. The remote manifest still
    /// describes the pre-pass state, so without the pushed log the next pass
    /// recomputes the same upload list and re-sends the request bodies.
    #[tokio::test]
    async fn interrupted_upload_resumes_without_resending() {
        let store = MemStore::default();
        let cfg = SyncBackendConfig::default();
        let vault = temp_vault("resume");
        fs::create_dir_all(vault.join("notes")).unwrap();
        for name in ["a", "b", "c", "d"] {
            fs::write(
                vault.join("notes").join(format!("{name}.md")),
                format!("# {name}\n"),
            )
            .unwrap();
        }

        // The budget covers vault.json plus the first two blobs; the third is
        // refused, so HEAD never advances and nothing is published.
        store.put_budget.store(3, Ordering::Relaxed);
        assert!(run_sync(&vault, &cfg, &store, noop_progress())
            .await
            .is_err());

        store.put_budget.store(usize::MAX, Ordering::Relaxed);
        let before = store.counts();
        let resumed = run_sync(&vault, &cfg, &store, noop_progress())
            .await
            .unwrap();
        let (gets, puts) = store.counts();
        assert_eq!((resumed.version, resumed.uploaded), (1, 2));
        assert_eq!(puts - before.1, 4, "2 remaining blobs + manifest + HEAD");
        assert_eq!(gets - before.0, 2, "vault.json + HEAD");

        // The published manifest covers every blob, so the log is spent and
        // the next pass short-circuits.
        let before = store.counts();
        run_sync(&vault, &cfg, &store, noop_progress())
            .await
            .unwrap();
        let (gets, puts) = store.counts();
        assert_eq!((gets - before.0, puts - before.1), (1, 0));

        let _ = fs::remove_dir_all(&vault);
    }

    // Actual engine round trips, including divergent mark files and races.
    include!("engine_marks_tests.rs");
    include!("engine_availability_tests.rs");
    include!("engine_retarget_tests.rs");
    include!("ink_durability_tests.rs");

    #[test]
    fn merge_one_sided_changes() {
        let base = map(&[("a.md", entry("h1", 1)), ("b.pdf", entry("h2", 1))]);
        let local = map(&[("a.md", entry("h1-local", 2)), ("b.pdf", entry("h2", 1))]);
        let remote = map(&[("a.md", entry("h1", 1))]); // remote deleted b.pdf

        let plan = merge(
            &base,
            &local,
            &remote,
            &snapshot::SyncScope::all(),
            &snapshot::SyncScope::all(),
        );
        assert_eq!(plan.merged["a.md"].hash, "h1-local");
        assert!(!plan.merged.contains_key("b.pdf"));
        assert_eq!(plan.delete_local, vec!["b.pdf"]);
        assert!(plan.downloads.is_empty());
    }

    #[test]
    fn merge_divergent_md_keeps_both_versions() {
        let base = map(&[("n.md", entry("h0", 1))]);
        let local = map(&[("n.md", entry("hl", 10))]);
        let remote = map(&[("n.md", entry("hr", 20))]);

        let plan = merge(
            &base,
            &local,
            &remote,
            &snapshot::SyncScope::all(),
            &snapshot::SyncScope::all(),
        );
        // Remote newer: it takes the path, local survives as conflict copy.
        assert_eq!(plan.merged["n.md"].hash, "hr");
        assert_eq!(plan.preserve_local.len(), 1);
        let copy = &plan.preserve_local[0].1;
        assert!(copy.contains("(conflict"));
        assert_eq!(plan.merged[copy].hash, "hl");
    }

    #[test]
    fn merge_divergent_research_tables_preserves_both_readable_files() {
        for (local_time, remote_time) in [(10, 20), (20, 10)] {
            let path = "data/results.table.json";
            let plan = merge(
                &map(&[(path, entry("base", 1))]),
                &map(&[(path, entry("local", local_time))]),
                &map(&[(path, entry("remote", remote_time))]),
                &snapshot::SyncScope::all(),
                &snapshot::SyncScope::all(),
            );
            assert_eq!(plan.merged.len(), 2);
            let copy = plan.merged.keys().find(|key| key.as_str() != path).unwrap();
            assert!(copy.ends_with(".table.json"));
            assert!(copy.contains(" (conflict "));
            assert_ne!(plan.merged[path].hash, plan.merged[copy].hash);
        }
    }

    #[test]
    fn merge_divergent_binary_is_lww() {
        let base = map(&[("m.json", entry("h0", 1))]);
        let local = map(&[("m.json", entry("hl", 30))]);
        let remote = map(&[("m.json", entry("hr", 20))]);

        let plan = merge(
            &base,
            &local,
            &remote,
            &snapshot::SyncScope::all(),
            &snapshot::SyncScope::all(),
        );
        assert_eq!(plan.merged["m.json"].hash, "hl");
        assert!(plan.downloads.is_empty());
        assert!(plan.preserve_local.is_empty());
    }

    #[test]
    fn merge_delete_vs_modify_keeps_modification() {
        let base = map(&[("k.md", entry("h0", 1))]);
        let local = map(&[]); // deleted locally
        let remote = map(&[("k.md", entry("hr", 5))]); // modified remotely

        let plan = merge(
            &base,
            &local,
            &remote,
            &snapshot::SyncScope::all(),
            &snapshot::SyncScope::all(),
        );
        assert_eq!(plan.merged["k.md"].hash, "hr");
        assert_eq!(plan.downloads.len(), 1);
    }

    fn no_pdf_scope() -> snapshot::SyncScope {
        snapshot::SyncScope {
            pdf: false,
            ..snapshot::SyncScope::default()
        }
    }

    #[test]
    fn merge_remote_filtered_path_is_not_a_delete() {
        // The remote publisher syncs without PDFs: its manifest lacks the
        // entry, but that absence must not delete the local file.
        let base = map(&[("papers/x/x.pdf", entry("h0", 1))]);
        let local = map(&[("papers/x/x.pdf", entry("h0", 1))]);
        let remote = map(&[]);

        let plan = merge(
            &base,
            &local,
            &remote,
            &snapshot::SyncScope::all(),
            &no_pdf_scope(),
        );
        assert_eq!(plan.merged["papers/x/x.pdf"].hash, "h0");
        assert!(plan.delete_local.is_empty());
        assert!(plan.downloads.is_empty());
    }

    #[test]
    fn merge_locally_filtered_path_is_inert() {
        // This device skips PDFs: the remote entry must not be downloaded,
        // and the base entry survives in the merged manifest.
        let base = map(&[("papers/x/x.pdf", entry("h0", 1))]);
        let local = map(&[]); // filtered out of the scan
        let remote = map(&[("papers/x/x.pdf", entry("h0", 1))]);

        let plan = merge(
            &base,
            &local,
            &remote,
            &no_pdf_scope(),
            &snapshot::SyncScope::all(),
        );
        assert_eq!(plan.merged["papers/x/x.pdf"].hash, "h0");
        assert!(plan.downloads.is_empty());
        assert!(plan.delete_local.is_empty());
    }

    #[test]
    fn merge_local_delete_wins_over_blind_remote() {
        // This device sees PDFs and deleted one; the remote publisher was
        // blind to the category, so the delete propagates (entry dropped).
        let base = map(&[("papers/x/x.pdf", entry("h0", 1))]);
        let local = map(&[]); // genuinely deleted
        let remote = map(&[]); // blind publisher

        let plan = merge(
            &base,
            &local,
            &remote,
            &snapshot::SyncScope::all(),
            &no_pdf_scope(),
        );
        assert!(!plan.merged.contains_key("papers/x/x.pdf"));
        assert!(plan.downloads.is_empty());
        assert!(plan.delete_local.is_empty());
    }

    #[test]
    fn validate_manifest_rejects_unsafe_paths_and_hashes() {
        let ok_hash = "a".repeat(64);
        let valid = Manifest {
            version: 1,
            files: map(&[("papers/x/NOTES.md", entry(&ok_hash, 1))]),
            scope: snapshot::SyncScope::all(),
        };
        assert!(validate_manifest(&valid).is_ok());

        for bad in [
            "../evil.md",
            "/abs.md",
            "a\\b.md",
            "a//b.md",
            "a/./b.md",
            "a/../b.md",
            "",
        ] {
            let m = Manifest {
                version: 1,
                files: map(&[(bad, entry(&ok_hash, 1))]),
                scope: snapshot::SyncScope::all(),
            };
            assert!(
                validate_manifest(&m).is_err(),
                "path {bad:?} must be rejected"
            );
        }

        for bad_hash in ["", "abc", &"A".repeat(64), &"g".repeat(64), &"a".repeat(63)] {
            let m = Manifest {
                version: 1,
                files: map(&[("n.md", entry(bad_hash, 1))]),
                scope: snapshot::SyncScope::all(),
            };
            assert!(
                validate_manifest(&m).is_err(),
                "hash {bad_hash:?} must be rejected"
            );
        }
    }

    #[test]
    fn gunzip_limited_enforces_cap() {
        let packed = gzip(&vec![0u8; 4096]).unwrap();
        assert_eq!(gunzip_limited(&packed, 4096).unwrap().len(), 4096);
        assert!(gunzip_limited(&packed, 4095).is_err());
    }

    /// The remote layout's artifact names must stay covered by the shared
    /// ignore list, or a mirrored store would leak back into the blob store.
    #[test]
    fn store_artifact_names_cover_the_remote_layout() {
        use crate::features::vault::tree::SYNC_STORE_ARTIFACT_NAMES;
        for key in [HEAD_KEY, VAULT_KEY, "blobs", "manifests"] {
            assert!(
                SYNC_STORE_ARTIFACT_NAMES.contains(&key),
                "{key} missing from SYNC_STORE_ARTIFACT_NAMES"
            );
        }
    }

    /// Full two-device round trip against a live S3 endpoint.
    ///
    /// ```sh
    /// docker run -d --rm --name minio -p 19000:9000 \
    ///   -e MINIO_ROOT_USER=testkey -e MINIO_ROOT_PASSWORD=testsecret \
    ///   minio/minio server /data
    /// docker exec minio mc alias set local http://127.0.0.1:9000 testkey testsecret
    /// docker exec minio mc mb local/vault-test
    /// cargo test -p agentero --lib features::sync::engine -- --ignored
    /// ```
    #[tokio::test]
    #[ignore = "requires a local MinIO (see doc comment)"]
    async fn two_device_roundtrip_against_minio() {
        use crate::features::sync::config::SyncBackendConfig;
        use uuid::Uuid;

        let cfg = SyncBackendConfig {
            endpoint: std::env::var("AGENTERO_SYNC_TEST_ENDPOINT")
                .unwrap_or_else(|_| "http://127.0.0.1:19000".into()),
            region: "us-east-1".into(),
            bucket: "vault-test".into(),
            prefix: format!("it-{}", Uuid::new_v4()),
            access_key: "testkey".into(),
            secret_key: "testsecret".into(),
            force_path_style: true,
            auto_sync: false,
            interval_minutes: 30,
            conditional_writes: true,
            scope: snapshot::SyncScope::all(),
            ..SyncBackendConfig::default()
        };
        let noop: &(dyn Fn(&str, usize, usize) + Send + Sync) = &|_, _, _| {};

        let tmp = std::env::temp_dir().join(format!("agentero-sync-it-{}", Uuid::new_v4()));
        let (a, b) = (tmp.join("a"), tmp.join("b"));
        fs::create_dir_all(a.join("papers/x")).unwrap();
        fs::create_dir_all(&b).unwrap();
        fs::write(a.join("papers/x/NOTES.md"), "# x\n").unwrap();
        fs::write(a.join("papers/x/metadata.json"), r#"{"id":"x"}"#).unwrap();

        // A publishes, empty B joins and receives everything. B adds nothing,
        // so it adopts version 1 rather than republishing it.
        let up = sync_vault(&a, &cfg, noop).await.expect("sync A");
        assert_eq!((up.version, up.uploaded), (1, 2));
        let down = sync_vault(&b, &cfg, noop).await.expect("sync B");
        assert_eq!((down.version, down.downloaded), (1, 2));
        assert_eq!(
            fs::read_to_string(b.join("papers/x/NOTES.md")).unwrap(),
            "# x\n"
        );

        // B edits; A picks it up.
        fs::write(b.join("papers/x/NOTES.md"), "# x\nedited on B\n").unwrap();
        sync_vault(&b, &cfg, noop).await.expect("sync B edit");
        let pull = sync_vault(&a, &cfg, noop).await.expect("sync A pull");
        assert_eq!(pull.downloaded, 1);
        assert!(fs::read_to_string(a.join("papers/x/NOTES.md"))
            .unwrap()
            .contains("edited on B"));

        // Divergent edits on both → conflict copy, then both converge.
        fs::write(a.join("papers/x/NOTES.md"), "# x\nA version\n").unwrap();
        fs::write(b.join("papers/x/NOTES.md"), "# x\nB version\n").unwrap();
        sync_vault(&a, &cfg, noop).await.expect("sync A divergent");
        let conflicted = sync_vault(&b, &cfg, noop).await.expect("sync B divergent");
        assert_eq!(conflicted.conflict_copies.len(), 1);
        sync_vault(&a, &cfg, noop).await.expect("sync A converge");
        // Compare content only: mtimes legitimately differ across devices.
        let hashes = |vault: &Path| -> BTreeMap<String, String> {
            snapshot::scan_vault(vault, &Manifest::default(), &snapshot::SyncScope::all())
                .unwrap()
                .into_iter()
                .map(|(k, v)| (k, v.hash))
                .collect()
        };
        assert_eq!(hashes(&a), hashes(&b), "both devices converge");

        let _ = fs::remove_dir_all(&tmp);
    }

    /// Full two-device round trip against a live WebDAV endpoint.
    ///
    /// ```sh
    /// AGENTERO_SYNC_WEBDAV_TEST_URL=https://dav.jianguoyun.com/dav/ \
    /// AGENTERO_SYNC_WEBDAV_TEST_USERNAME=user@example.com \
    /// AGENTERO_SYNC_WEBDAV_TEST_PASSWORD=app-password \
    /// cargo test -p agentero --lib integration::sync::engine -- --ignored
    /// ```
    ///
    /// Uses a fresh `agentero-it-<hex>` directory per run (created via MKCOL;
    /// Nutstore caps top-level folder name length, so keep it short); clean
    /// it up afterwards by deleting the directory (Nutstore requires deleting
    /// files before their parent collection).
    ///
    /// Set `AGENTERO_SYNC_WEBDAV_TEST_DEGRADED=1` to opt into the degraded
    /// protocol (required for servers that ignore conditional headers, e.g.
    /// fnOS built-in WebDAV; the probe result is printed either way).
    #[tokio::test]
    #[ignore = "requires a live WebDAV server (see doc comment)"]
    async fn two_device_roundtrip_against_webdav() {
        use crate::features::sync::config::SyncBackendKind;
        use uuid::Uuid;

        let base = std::env::var("AGENTERO_SYNC_WEBDAV_TEST_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:9001".into());
        let nonce = &Uuid::new_v4().simple().to_string()[..12];
        let mut cfg = SyncBackendConfig {
            backend: SyncBackendKind::Webdav,
            webdav_url: format!("{}/agentero-it-{nonce}", base.trim_end_matches('/')),
            webdav_username: std::env::var("AGENTERO_SYNC_WEBDAV_TEST_USERNAME")
                .unwrap_or_default(),
            webdav_password: std::env::var("AGENTERO_SYNC_WEBDAV_TEST_PASSWORD")
                .unwrap_or_default(),
            auto_sync: false,
            interval_minutes: 30,
            conditional_writes: true,
            scope: snapshot::SyncScope::all(),
            ..SyncBackendConfig::default()
        };
        // Mirror sync_configure: reachability (creates the directory) plus
        // the conditional-write probe that seeds the persisted flag.
        let probed = test_connection(Path::new("."), &cfg)
            .await
            .expect("webdav test_connection");
        cfg.conditional_writes = probed;
        cfg.allow_degraded_sync = std::env::var("AGENTERO_SYNC_WEBDAV_TEST_DEGRADED")
            .map(|v| v == "1")
            .unwrap_or(false);
        eprintln!("webdav conditional writes: {probed}");

        let noop: &(dyn Fn(&str, usize, usize) + Send + Sync) = &|_, _, _| {};

        let tmp = std::env::temp_dir().join(format!("agentero-sync-it-{}", Uuid::new_v4()));
        let (a, b) = (tmp.join("a"), tmp.join("b"));
        fs::create_dir_all(a.join("papers/x")).unwrap();
        fs::create_dir_all(&b).unwrap();
        fs::write(a.join("papers/x/NOTES.md"), "# x\n").unwrap();
        fs::write(a.join("papers/x/metadata.json"), r#"{"id":"x"}"#).unwrap();

        // A publishes, empty B joins and receives everything. B adds nothing,
        // so it adopts version 1 rather than republishing it.
        let up = sync_vault(&a, &cfg, noop).await.expect("sync A");
        assert_eq!((up.version, up.uploaded), (1, 2));
        let down = sync_vault(&b, &cfg, noop).await.expect("sync B");
        assert_eq!((down.version, down.downloaded), (1, 2));
        assert_eq!(
            fs::read_to_string(b.join("papers/x/NOTES.md")).unwrap(),
            "# x\n"
        );

        // B edits; A picks it up.
        fs::write(b.join("papers/x/NOTES.md"), "# x\nedited on B\n").unwrap();
        sync_vault(&b, &cfg, noop).await.expect("sync B edit");
        let pull = sync_vault(&a, &cfg, noop).await.expect("sync A pull");
        assert_eq!(pull.downloaded, 1);
        assert!(fs::read_to_string(a.join("papers/x/NOTES.md"))
            .unwrap()
            .contains("edited on B"));

        // Divergent edits on both → conflict copy, then both converge.
        fs::write(a.join("papers/x/NOTES.md"), "# x\nA version\n").unwrap();
        fs::write(b.join("papers/x/NOTES.md"), "# x\nB version\n").unwrap();
        sync_vault(&a, &cfg, noop).await.expect("sync A divergent");
        let conflicted = sync_vault(&b, &cfg, noop).await.expect("sync B divergent");
        assert_eq!(conflicted.conflict_copies.len(), 1);
        sync_vault(&a, &cfg, noop).await.expect("sync A converge");
        // Compare content only: mtimes legitimately differ across devices.
        let hashes = |vault: &Path| -> BTreeMap<String, String> {
            snapshot::scan_vault(vault, &Manifest::default(), &snapshot::SyncScope::all())
                .unwrap()
                .into_iter()
                .map(|(k, v)| (k, v.hash))
                .collect()
        };
        assert_eq!(hashes(&a), hashes(&b), "both devices converge");

        let _ = fs::remove_dir_all(&tmp);
    }
}
