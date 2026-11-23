//! Sync commands. Long-running `sync_now` broadcasts `sync:state` /
//! `sync:progress` events so the settings pane (and later a status bar
//! indicator) can follow along from any window.

use super::config::{self, SyncBackendConfig, VaultSyncConfig};
use super::credentials::{self, CredentialStatus, SyncCredentialState, TargetStatus};
use super::engine::{self, SyncOutcome};
use super::local;
use super::snapshot;
use super::SyncService;
use crate::core::error::{map_err, ApiResult, AppError};
use agentero_core::features::sync::availability::{self, DownloadPolicy, PaperAvailability};
use agentero_core::features::sync::conflicts::{self, ConflictResolution, PendingConflicts};
use agentero_core::features::sync::lock::SyncLock;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
#[cfg(mobile)]
use tauri::Manager;
use tauri::{AppHandle, Emitter, State};

#[derive(Debug, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SyncVaultArgs {
    pub vault_path: String,
}

#[derive(Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SyncConfigureArgs {
    pub vault_path: String,
    pub config: SyncBackendConfig,
    /// Read-only mirrors published after the primary pass. The primary stays
    /// the only backend that pulls and merges.
    #[serde(default)]
    pub mirrors: Vec<SyncBackendConfig>,
}

/// One read-only mirror and its last pass result.
#[derive(Debug, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SyncMirrorStatus {
    pub config: SyncBackendConfig,
    pub credential_state: SyncCredentialState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_sync_at: Option<String>,
    pub last_version: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

#[derive(Debug, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    pub configured: bool,
    pub credential_state: SyncCredentialState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<SyncBackendConfig>,
    pub running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_sync_at: Option<String>,
    pub last_version: u64,
    /// Divergent edits awaiting confirmation under the ask policy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_conflicts: Option<PendingConflicts>,
    /// Mirrors in configuration order; empty for a single-backend binding.
    #[serde(default)]
    pub mirrors: Vec<SyncMirrorStatus>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SyncStateEvent<'a> {
    vault_path: &'a str,
    status: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    /// Stable domain code of `error` (e.g. `sync_no_conditional_writes`) so
    /// listeners can localize without matching message text.
    #[serde(skip_serializing_if = "Option::is_none")]
    error_code: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SyncProgressEvent<'a> {
    vault_path: &'a str,
    phase: &'a str,
    current: usize,
    total: usize,
}

/// Per-mirror pass state. Progress phases stay vault-level, so a mirror run is
/// reported as one `started` → `published` | `error` pair instead.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SyncMirrorEvent<'a> {
    vault_path: &'a str,
    /// Zero-based position in the vault's mirror list (matches
    /// `SyncStatus.mirrors[]` while the saved binding is unchanged).
    index: usize,
    /// `SyncBackendConfig::remote_identity` — matches `SyncStatus.mirrors[].config`.
    identity: &'a str,
    backend: &'a str,
    /// `started` | `published` | `error`.
    status: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a str>,
}

fn backend_label(backend: config::SyncBackendKind) -> &'static str {
    match backend {
        config::SyncBackendKind::S3 => "s3",
        config::SyncBackendKind::Webdav => "webdav",
        config::SyncBackendKind::Git => "git",
        config::SyncBackendKind::Agentero => "agentero",
    }
}

pub fn account_api_origin() -> String {
    let local = cfg!(debug_assertions) && option_env!("AGENTERO_LOCAL_AUTH") == Some("1");
    if local {
        "http://localhost:8788".into()
    } else {
        "https://api.agentero.app".into()
    }
}

pub async fn session_bearer(app: &AppHandle) -> Option<String> {
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        use tauri_plugin_better_auth::BetterAuthExt;
        return app
            .better_auth()
            .bearer_token()
            .await
            .filter(|token| !token.trim().is_empty());
    }
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        let _ = app;
        None
    }
}

async fn attach_agentero_session(app: &AppHandle, cfg: &mut SyncBackendConfig) {
    if cfg.backend != super::config::SyncBackendKind::Agentero {
        return;
    }
    if cfg.agentero_api.trim().is_empty() {
        cfg.agentero_api = account_api_origin();
    }
    if let Some(token) = session_bearer(app).await {
        cfg.agentero_token = token;
    }
}

pub(super) fn vault_dir(app: &AppHandle, vault_path: &str) -> Result<PathBuf, AppError> {
    let trimmed = vault_path.trim();
    if trimmed.is_empty() || trimmed.starts_with("remote:") {
        return Err(AppError::message("sync requires a local vault"));
    }
    #[cfg(mobile)]
    {
        let expected = app
            .path()
            .app_data_dir()
            .map_err(|_| AppError::message("App private directory unavailable"))?
            .join("Vault")
            .canonicalize()?;
        let requested = std::fs::canonicalize(trimmed)?;
        if requested != expected {
            return Err(AppError::domain(
                "sync_private_vault_required",
                "Tablet sync requires the app private Vault",
            ));
        }
    }
    #[cfg(desktop)]
    let _ = app;
    crate::core::fs::resolve_vault(trimmed)
}

fn prepare_download_policy(dir: &std::path::Path) -> Result<(), AppError> {
    #[cfg(mobile)]
    availability::initialize_policy(
        dir,
        DownloadPolicy {
            mode: availability::DownloadMode::OnDemand,
            pinned_paper_ids: Vec::new(),
        },
    )?;
    #[cfg(desktop)]
    let _ = dir;
    Ok(())
}

/// Current sync binding + last-pass info for the settings pane.
#[tauri::command]
#[specta::specta]
pub async fn sync_get_status(
    app: AppHandle,
    args: SyncVaultArgs,
    service: State<'_, SyncService>,
) -> Result<ApiResult<SyncStatus>, String> {
    let inner = async {
        let key = args.vault_path.trim();
        let dir = vault_dir(&app, key)?;
        let status = credentials::status(&app, key).await?;
        let pending_conflicts = if status
            .primary
            .as_ref()
            .is_some_and(|target| asks_for_conflicts(&dir, &target.config))
        {
            conflicts::read_pending(&dir)?
        } else {
            None
        };
        sync_status(&dir, status, service.is_running(key), pending_conflicts)
    };
    Ok(match inner.await {
        Ok(status) => ApiResult::ok(status),
        Err(error) => map_err(error),
    })
}

fn sync_status(
    dir: &std::path::Path,
    status: CredentialStatus,
    running: bool,
    pending_conflicts: Option<PendingConflicts>,
) -> Result<SyncStatus, AppError> {
    let meta = local::read_meta(dir)?;
    let (config, credential_state) = match status.primary {
        Some(TargetStatus { config, state }) => (Some(config), state),
        None => (None, SyncCredentialState::NotConfigured),
    };
    let mirrors = status
        .mirrors
        .into_iter()
        .map(|target| {
            let state = engine::mirror_state(dir, &target.config);
            SyncMirrorStatus {
                config: target.config,
                credential_state: target.state,
                last_sync_at: state.last_sync_at,
                last_version: state.last_version,
                last_error: state.last_error,
            }
        })
        .collect();
    Ok(SyncStatus {
        configured: config.is_some(),
        config,
        credential_state,
        running,
        last_sync_at: meta.last_sync_at,
        last_version: meta.last_version,
        pending_conflicts,
        mirrors,
    })
}

/// Validate and probe before publishing a protected credential reference.
#[tauri::command]
#[specta::specta]
pub async fn sync_configure(
    app: AppHandle,
    service: State<'_, SyncService>,
    args: SyncConfigureArgs,
) -> Result<ApiResult<SyncStatus>, String> {
    let key = args.vault_path.trim().to_string();
    let inner = async {
        let dir = vault_dir(&app, &key)?;
        prepare_download_policy(&dir)?;
        let _lease = service
            .try_begin(&key)
            .ok_or_else(|| AppError::message("sync already running"))?;
        let lock = SyncLock::acquire(&dir)?;
        let previous = config::try_get(&key)?;
        let mut binding = VaultSyncConfig {
            primary: args.config.normalized(),
            mirrors: args
                .mirrors
                .into_iter()
                .map(SyncBackendConfig::normalized)
                .collect(),
        };
        attach_agentero_session(&app, &mut binding.primary).await;
        let keep_review = asks_for_conflicts(&dir, &binding.primary)
            && previous.as_ref().is_some_and(|saved| {
                saved.primary.conflict_policy == binding.primary.conflict_policy
                    && saved.primary.scope == binding.primary.scope
                    && saved.primary.remote_identity() == binding.primary.remote_identity()
            });
        if cfg!(mobile) && !keep_review {
            ensure_no_pending_review(&dir)?;
        }
        let configured = credentials::configure(&app, &key, &dir, binding).await?;
        let pending_conflicts = if keep_review {
            conflicts::read_pending(&dir)?
        } else {
            conflicts::clear_locked(&lock)?;
            None
        };
        sync_status(
            &dir,
            CredentialStatus::configured(&configured.binding, configured.protected),
            false,
            pending_conflicts,
        )
    };
    let result = inner.await;
    if result.is_ok() {
        service.restart_scheduler(&app, &key);
    }
    Ok(match result {
        Ok(status) => ApiResult::ok(status),
        Err(error) => map_err(error),
    })
}

/// Clear protected generations before reporting a disconnected binding.
#[tauri::command]
#[specta::specta]
pub async fn sync_disconnect(
    app: AppHandle,
    service: State<'_, SyncService>,
    args: SyncVaultArgs,
) -> Result<ApiResult<()>, String> {
    let key = args.vault_path.trim().to_string();
    let inner = async {
        let dir = vault_dir(&app, &key)?;
        let _lease = service
            .try_begin(&key)
            .ok_or_else(|| AppError::message("sync already running"))?;
        let lock = SyncLock::try_acquire(&dir)?
            .ok_or_else(|| AppError::message("another vault sync is running"))?;
        if cfg!(mobile) {
            ensure_no_pending_review(&dir)?;
        }
        credentials::clear(&app, &key).await?;
        local::clear(&lock)?;
        Ok::<_, AppError>(())
    };
    Ok(match inner.await {
        Ok(()) => {
            service.stop_scheduler(&key);
            ApiResult::ok(())
        }
        Err(error) => map_err(error),
    })
}

#[derive(Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SyncDownloadPolicyArgs {
    pub vault_path: String,
    pub policy: DownloadPolicy,
}
#[derive(Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SyncPaperArgs {
    pub vault_path: String,
    pub paper_id: String,
}

#[tauri::command]
#[specta::specta]
pub async fn sync_get_download_policy(
    app: AppHandle,
    args: SyncVaultArgs,
) -> Result<ApiResult<DownloadPolicy>, String> {
    Ok(
        match vault_dir(&app, &args.vault_path).and_then(|dir| {
            prepare_download_policy(&dir)?;
            availability::read_policy(&dir)
        }) {
            Ok(value) => ApiResult::ok(value),
            Err(error) => map_err(error),
        },
    )
}
#[tauri::command]
#[specta::specta]
pub async fn sync_set_download_policy(
    app: AppHandle,
    service: State<'_, SyncService>,
    args: SyncDownloadPolicyArgs,
) -> Result<ApiResult<DownloadPolicy>, String> {
    let result = vault_dir(&app, &args.vault_path)
        .and_then(|dir| availability::set_policy(&dir, args.policy));
    if result.is_ok() {
        service.notify_path_changed(&app, std::path::Path::new(args.vault_path.trim()));
    }
    Ok(match result {
        Ok(value) => ApiResult::ok(value),
        Err(error) => map_err(error),
    })
}
#[tauri::command]
#[specta::specta]
pub async fn sync_list_paper_availability(
    app: AppHandle,
    args: SyncVaultArgs,
) -> Result<ApiResult<Vec<PaperAvailability>>, String> {
    Ok(
        match vault_dir(&app, &args.vault_path).and_then(|dir| availability::list_papers(&dir)) {
            Ok(value) => ApiResult::ok(value),
            Err(error) => map_err(error),
        },
    )
}
#[tauri::command]
#[specta::specta]
pub async fn sync_fetch_paper(
    app: AppHandle,
    service: State<'_, SyncService>,
    args: SyncPaperArgs,
) -> Result<ApiResult<SyncOutcome>, String> {
    Ok(
        match perform_sync_task(
            &app,
            &service,
            args.vault_path.trim(),
            SyncTask::FetchPaper(&args.paper_id),
        )
        .await
        {
            Ok(value) => ApiResult::ok(value),
            Err(error) => map_err(error),
        },
    )
}
#[tauri::command]
#[specta::specta]
pub async fn sync_evict_paper(
    app: AppHandle,
    service: State<'_, SyncService>,
    args: SyncPaperArgs,
) -> Result<ApiResult<usize>, String> {
    let key = args.vault_path.trim();
    let result = (|| {
        let dir = vault_dir(&app, key)?;
        let _lease = service
            .try_begin(key)
            .ok_or_else(|| AppError::message("sync already running"))?;
        availability::evict_paper(&dir, &args.paper_id)
    })();
    Ok(match result {
        Ok(value) => ApiResult::ok(value),
        Err(error) => map_err(error),
    })
}
/// Explicit hints supplement native writes/imports; they never bypass auto-sync preferences.
#[tauri::command]
#[specta::specta]
pub async fn sync_notify_change(
    app: AppHandle,
    service: State<'_, SyncService>,
    args: SyncVaultArgs,
) -> Result<ApiResult<()>, String> {
    Ok(match vault_dir(&app, &args.vault_path) {
        Ok(dir) => {
            service.notify_path_changed(&app, &dir);
            ApiResult::ok(())
        }
        Err(error) => map_err(error),
    })
}

/// Local disk usage per bulky-asset category (bytes), for the sync-scope UI.
/// The shared snapshot classifier covers all five categories (including the
/// mobile ink / translated-PDF buckets); the app-aware vault guard keeps the
/// tablet private-Vault boundary.
#[tauri::command]
#[specta::specta]
pub async fn sync_scope_sizes(
    app: AppHandle,
    args: SyncVaultArgs,
) -> Result<ApiResult<snapshot::SyncScopeSizes>, String> {
    let inner = async {
        let dir = vault_dir(&app, &args.vault_path)?;
        tokio::task::spawn_blocking(move || snapshot::scope_sizes(&dir))
            .await
            .map_err(|e| AppError::message(format!("scope sizes: {e}")))?
    };
    Ok(match inner.await {
        Ok(sizes) => ApiResult::ok(sizes),
        Err(e) => map_err(e),
    })
}

/// One full sync pass shared by the `sync_now` command and the auto-sync
/// scheduler (scan → merge → publish → apply).
pub async fn perform_sync(
    app: &AppHandle,
    service: &SyncService,
    vault_key: &str,
) -> Result<SyncOutcome, AppError> {
    perform_sync_task(app, service, vault_key, SyncTask::Sync).await
}

enum SyncTask<'a> {
    Sync,
    FetchPaper(&'a str),
    Resolve(SyncResolveConflictsArgs),
}

async fn perform_sync_task(
    app: &AppHandle,
    service: &SyncService,
    vault_key: &str,
    task: SyncTask<'_>,
) -> Result<SyncOutcome, AppError> {
    let dir = vault_dir(app, vault_key)?;
    prepare_download_policy(&dir)?;
    let lease = service
        .try_begin(vault_key)
        .ok_or_else(|| AppError::message("sync already running"))?;
    let lock = SyncLock::acquire(&dir)?;
    let mut binding = credentials::load(app, vault_key)
        .await?
        .ok_or_else(|| AppError::message("sync is not configured"))?;
    attach_agentero_session(app, &mut binding.primary).await;
    emit_state(app, vault_key, "syncing", None, None);
    let progress_app = app.clone();
    let progress_key = vault_key.to_string();
    let progress = move |phase: &str, current: usize, total: usize| {
        let _ = progress_app.emit(
            "sync:progress",
            SyncProgressEvent {
                vault_path: &progress_key,
                phase,
                current,
                total,
            },
        );
    };
    let mirror_watch = |index: usize,
                        mirror: &SyncBackendConfig,
                        phase: engine::MirrorPhase,
                        error: Option<&str>| {
        let status = match phase {
            engine::MirrorPhase::Started => "started",
            engine::MirrorPhase::Published => "published",
            engine::MirrorPhase::Failed => "error",
        };
        let _ = app.emit(
            "sync:mirror",
            SyncMirrorEvent {
                vault_path: vault_key,
                index,
                identity: &credentials::masked_status_config(mirror).remote_identity(),
                backend: backend_label(mirror.backend),
                status,
                error,
            },
        );
    };
    let reason = if matches!(task, SyncTask::FetchPaper(_)) {
        crate::features::mobile_background::BackgroundReason::Transfer
    } else {
        crate::features::mobile_background::BackgroundReason::Sync
    };
    let result = crate::features::mobile_background::with_work(app, reason, async {
        match task {
            SyncTask::FetchPaper(paper) => {
                availability::fetch_paper_locked(&lock, &binding.primary, paper, &progress).await
            }
            SyncTask::Sync => {
                engine::sync_all_locked(&lock, &binding, &progress, Some(&mirror_watch)).await
            }
            SyncTask::Resolve(args) => {
                let outcome = engine::resolve_conflicts_locked(
                    &lock,
                    &binding.primary,
                    &args.review_id,
                    args.default_resolution,
                    args.resolutions,
                    &progress,
                )
                .await?;
                engine::publish_mirrors_locked(&lock, &binding, &progress, Some(&mirror_watch))
                    .await;
                Ok(outcome)
            }
        }
    })
    .await;
    // Terminal event consumers immediately re-read status; release first.
    drop(lock);
    drop(lease);
    match result {
        Ok(outcome) => {
            emit_state(app, vault_key, "idle", None, None);
            Ok(outcome)
        }
        Err(e) => {
            if e.code() == conflicts::PENDING_CODE {
                emit_state(app, vault_key, "conflict", None, None);
            } else {
                emit_state(
                    app,
                    vault_key,
                    "error",
                    Some(e.to_string()),
                    Some(e.code().to_string()),
                );
            }
            Err(e)
        }
    }
}

/// One full sync pass (manual trigger from the settings pane).
#[tauri::command]
#[specta::specta]
pub async fn sync_now(
    app: AppHandle,
    service: State<'_, SyncService>,
    args: SyncVaultArgs,
) -> Result<ApiResult<SyncOutcome>, String> {
    use crate::core::log_util::OpTimer;

    let vault_key = args.vault_path.trim().to_string();
    let op = OpTimer::start_with("sync_now", format!("vault={vault_key}"));
    match perform_sync(&app, &service, &vault_key).await {
        Ok(outcome) => {
            op.finish_ok_extra(format!(
                "version={} up={} down={} conflicts={}",
                outcome.version,
                outcome.uploaded,
                outcome.downloaded,
                outcome.conflict_copies.len()
            ));
            Ok(ApiResult::ok(outcome))
        }
        Err(e) => {
            op.finish_err(&e);
            Ok(map_err(e))
        }
    }
}

fn asks_for_conflicts(vault: &std::path::Path, config: &SyncBackendConfig) -> bool {
    config.conflict_policy == super::config::ConflictPolicy::Ask
        && config.backend != super::config::SyncBackendKind::Git
        && matches!(engine::uses_webdav_journal(vault, config), Ok(false))
}

fn ensure_no_pending_review(vault: &std::path::Path) -> Result<(), AppError> {
    if conflicts::read_pending(vault)?.is_some() {
        return Err(AppError::domain(
            conflicts::PENDING_CODE,
            "Resolve the pending sync conflicts before changing the connection",
        ));
    }
    Ok(())
}

/// Automatic sync waits only for unanswered reviews. A submitted review may
/// resume after a failure or restart, subject to the same version validation.
pub(super) fn has_pending_conflicts(app: &AppHandle, vault_path: &str) -> bool {
    let Ok(dir) = vault_dir(app, vault_path) else {
        return false;
    };
    super::config::try_get(vault_path)
        .ok()
        .flatten()
        .as_ref()
        .is_some_and(|binding| asks_for_conflicts(&dir, &binding.primary))
        && conflicts::read_pending(&dir).ok().flatten().is_some()
}

/// Persist the user's per-file conflict decisions and re-run the paused pass.
/// The override file is durable before the pass starts; a crash mid-pass leaves
/// it in place so the next attempt resumes the same decision.
#[tauri::command]
#[specta::specta]
pub async fn sync_resolve_conflicts(
    app: AppHandle,
    service: State<'_, SyncService>,
    args: SyncResolveConflictsArgs,
) -> Result<ApiResult<SyncOutcome>, String> {
    use crate::core::log_util::OpTimer;

    let vault_key = args.vault_path.trim().to_string();
    let op = OpTimer::start_with(
        "sync_resolve_conflicts",
        format!("vault={vault_key} resolutions={}", args.resolutions.len()),
    );
    match perform_sync_task(&app, &service, &vault_key, SyncTask::Resolve(args)).await {
        Ok(outcome) => {
            op.finish_ok_extra(format!(
                "version={} up={} down={} conflicts={}",
                outcome.version,
                outcome.uploaded,
                outcome.downloaded,
                outcome.conflict_copies.len()
            ));
            Ok(ApiResult::ok(outcome))
        }
        Err(e) => {
            op.finish_err(&e);
            Ok(map_err(e))
        }
    }
}

#[derive(Debug, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SyncResolveConflictsArgs {
    pub vault_path: String,
    pub review_id: String,
    /// Fallback for reviewed paths absent from `resolutions`.
    pub default_resolution: ConflictResolution,
    pub resolutions: std::collections::BTreeMap<String, ConflictResolution>,
}

#[derive(Debug, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SyncOverwriteArgs {
    pub vault_path: String,
    pub direction: engine::OverwriteDirection,
}

fn ensure_desktop_overwrite() -> Result<(), AppError> {
    if cfg!(mobile) {
        return Err(AppError::domain(
            "sync_overwrite_unavailable_mobile",
            "Manual sync overwrite is only available on desktop",
        ));
    }
    Ok(())
}

/// Read-only impact preview for the manual-overwrite confirmation dialog.
#[tauri::command]
#[specta::specta]
pub async fn sync_overwrite_preview(
    app: AppHandle,
    args: SyncOverwriteArgs,
) -> Result<ApiResult<engine::OverwritePreview>, String> {
    let key = args.vault_path.trim().to_string();
    let inner = async {
        ensure_desktop_overwrite()?;
        let dir = vault_dir(&app, &key)?;
        let _lock = SyncLock::acquire(&dir)?;
        let binding = credentials::load(&app, &key)
            .await?
            .ok_or_else(|| AppError::message("sync is not configured"))?;
        engine::overwrite_preview(&dir, &binding.primary, args.direction).await
    };
    Ok(match inner.await {
        Ok(preview) => ApiResult::ok(preview),
        Err(error) => map_err(error),
    })
}

/// Manual overwrite: `remoteToLocal` mirrors the remote HEAD onto disk
/// (discarding divergent local content); `localToRemote` publishes the local
/// scan as the new HEAD. The UI confirms with a preview before calling this.
#[tauri::command]
#[specta::specta]
pub async fn sync_overwrite(
    app: AppHandle,
    service: State<'_, SyncService>,
    args: SyncOverwriteArgs,
) -> Result<ApiResult<SyncOutcome>, String> {
    use crate::core::log_util::OpTimer;

    let vault_key = args.vault_path.trim().to_string();
    let op = OpTimer::start_with(
        "sync_overwrite",
        format!("vault={vault_key} direction={:?}", args.direction),
    );
    let inner = async {
        ensure_desktop_overwrite()?;
        let dir = vault_dir(&app, &vault_key)?;
        let lease = service
            .try_begin(&vault_key)
            .ok_or_else(|| AppError::message("sync already running"))?;
        let lock = SyncLock::acquire(&dir)?;
        let binding = credentials::load(&app, &vault_key)
            .await?
            .ok_or_else(|| AppError::message("sync is not configured"))?;
        emit_state(&app, &vault_key, "syncing", None, None);
        let progress_app = app.clone();
        let progress_key = vault_key.clone();
        let progress = move |phase: &str, current: usize, total: usize| {
            let _ = progress_app.emit(
                "sync:progress",
                SyncProgressEvent {
                    vault_path: &progress_key,
                    phase,
                    current,
                    total,
                },
            );
        };
        let result =
            engine::overwrite_locked(&lock, &binding.primary, args.direction, &progress).await;
        drop(lock);
        // Terminal event consumers immediately re-read status; release first.
        drop(lease);
        match &result {
            Ok(_) => emit_state(&app, &vault_key, "idle", None, None),
            Err(e) => emit_state(
                &app,
                &vault_key,
                "error",
                Some(e.to_string()),
                Some(e.code().to_string()),
            ),
        }
        result
    };
    match inner.await {
        Ok(outcome) => {
            op.finish_ok_extra(format!(
                "version={} up={} down={} deletedLocal={}",
                outcome.version, outcome.uploaded, outcome.downloaded, outcome.deleted_local
            ));
            Ok(ApiResult::ok(outcome))
        }
        Err(e) => {
            op.finish_err(&e);
            Ok(map_err(e))
        }
    }
}

#[derive(Debug, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SyncCloudUsageArgs {
    /// `user` or `organization`.
    pub subject: String,
    pub org_id: Option<String>,
}

#[derive(Debug, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SyncCloudUsage {
    pub signed_in: bool,
    pub used: u64,
    pub limit: i64,
    pub tier: String,
    pub plan_key: String,
}

fn signed_out_usage() -> SyncCloudUsage {
    SyncCloudUsage {
        signed_in: false,
        used: 0,
        limit: 0,
        tier: String::new(),
        plan_key: String::new(),
    }
}

/// Used and included bytes for the signed-in personal or team space.
/// The bearer stays in the keychain; the webview only sees the numbers.
#[tauri::command]
#[specta::specta]
pub async fn sync_cloud_usage(
    app: AppHandle,
    args: SyncCloudUsageArgs,
) -> Result<ApiResult<SyncCloudUsage>, String> {
    let Some(token) = session_bearer(&app).await else {
        return Ok(ApiResult::ok(signed_out_usage()));
    };
    let subject = if args.subject == "organization" {
        "organization"
    } else {
        "user"
    };
    if subject == "organization" && args.org_id.as_deref().unwrap_or("").trim().is_empty() {
        return Ok(ApiResult::ok(SyncCloudUsage {
            signed_in: true,
            ..signed_out_usage()
        }));
    }
    let mut url = format!("{}/api/sync/usage?subject={subject}", account_api_origin());
    if subject == "organization" {
        let id = args.org_id.unwrap_or_default();
        url.push_str("&id=");
        for byte in id.bytes() {
            match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    url.push(byte as char);
                }
                _ => url.push_str(&format!("%{byte:02X}")),
            }
        }
    }
    let client = match crate::core::http::client_builder().build() {
        Ok(client) => client,
        Err(e) => return Ok(map_err(AppError::message(e.to_string()))),
    };
    let response = match client.get(url).bearer_auth(token).send().await {
        Ok(response) => response,
        Err(e) => return Ok(map_err(AppError::message(e.to_string()))),
    };
    if response.status().as_u16() == 401 {
        return Ok(ApiResult::ok(signed_out_usage()));
    }
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Ok(map_err(AppError::message(format!(
            "cloud usage: {status} {}",
            body.chars().take(180).collect::<String>()
        ))));
    }
    let payload: serde_json::Value = match response.json().await {
        Ok(payload) => payload,
        Err(e) => return Ok(map_err(AppError::message(format!("cloud usage: {e}")))),
    };
    let data = payload.get("data").cloned().unwrap_or(payload);
    Ok(ApiResult::ok(SyncCloudUsage {
        signed_in: true,
        used: data.get("used").and_then(|v| v.as_u64()).unwrap_or(0),
        limit: data.get("limit").and_then(|v| v.as_i64()).unwrap_or(0),
        tier: data
            .get("tier")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        plan_key: data
            .get("planKey")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
    }))
}

fn emit_state(
    app: &AppHandle,
    vault_path: &str,
    status: &str,
    error: Option<String>,
    error_code: Option<String>,
) {
    let _ = app.emit(
        "sync:state",
        SyncStateEvent {
            vault_path,
            status,
            error,
            error_code,
        },
    );
}

/// Anti-drift: bind the owned `SyncStateEvent` / `SyncProgressEvent` mirrors
/// (in `app::events_contract`, feeding the `sync:state` / `sync:progress`
/// payload types in bindings.ts) to the private borrowed structs actually
/// emitted here. Borrowed fields (`&str`) cannot share compile-time type
/// identity with the owned mirror, so the binding is the serde shape (covers
/// `rename_all` + `skip_serializing_if`); the owned `usize` counters are also
/// type-bound.
#[cfg(test)]
mod events_contract_shape_tests {
    use super::{SyncMirrorEvent, SyncProgressEvent, SyncStateEvent};
    use crate::app::events_contract::{
        SyncMirrorEvent as MirrorSyncMirror, SyncProgressEvent as MirrorSyncProgress,
        SyncStateEvent as MirrorSyncState,
    };

    #[test]
    fn sync_state_mirror_matches_emit_payload_shape() {
        let real = SyncStateEvent {
            vault_path: "/vault",
            status: "configured",
            error: Some("offline".to_string()),
            error_code: Some("sync_unreachable".to_string()),
        };
        let mirror = MirrorSyncState {
            vault_path: "/vault".to_string(),
            status: "configured".to_string(),
            error: Some("offline".to_string()),
            error_code: Some("sync_unreachable".to_string()),
        };
        assert_eq!(
            serde_json::to_value(&real).unwrap(),
            serde_json::to_value(&mirror).unwrap(),
            "SyncStateEvent mirror drifted from the emitted payload"
        );
        let real_none = SyncStateEvent {
            vault_path: "/vault",
            status: "disconnected",
            error: None,
            error_code: None,
        };
        let mirror_none = MirrorSyncState {
            vault_path: "/vault".to_string(),
            status: "disconnected".to_string(),
            error: None,
            error_code: None,
        };
        assert_eq!(
            serde_json::to_value(&real_none).unwrap(),
            serde_json::to_value(&mirror_none).unwrap(),
            "SyncStateEvent mirror drifted from the emitted payload (None variant)"
        );
    }

    #[test]
    fn sync_progress_mirror_matches_emit_payload_shape() {
        let real = SyncProgressEvent {
            vault_path: "/vault",
            phase: "upload",
            current: 3,
            total: 10,
        };
        let mirror = MirrorSyncProgress {
            vault_path: "/vault".to_string(),
            phase: "upload".to_string(),
            current: 3,
            total: 10,
        };
        assert_eq!(
            serde_json::to_value(&real).unwrap(),
            serde_json::to_value(&mirror).unwrap(),
            "SyncProgressEvent mirror drifted from the emitted payload"
        );
        fn eq_type<T>(_: &T, _: &T) {}
        eq_type(&real.current, &mirror.current);
        eq_type(&real.total, &mirror.total);
    }

    #[test]
    fn sync_mirror_mirror_matches_emit_payload_shape() {
        let real = SyncMirrorEvent {
            vault_path: "/vault",
            index: 0,
            identity: "git|git@example.com:me/vault.git|main",
            backend: "git",
            status: "error",
            error: Some("push rejected"),
        };
        let mirror = MirrorSyncMirror {
            vault_path: "/vault".to_string(),
            index: 0,
            identity: "git|git@example.com:me/vault.git|main".to_string(),
            backend: "git".to_string(),
            status: "error".to_string(),
            error: Some("push rejected".to_string()),
        };
        assert_eq!(
            serde_json::to_value(&real).unwrap(),
            serde_json::to_value(&mirror).unwrap(),
            "SyncMirrorEvent mirror drifted from the emitted payload"
        );
        let real_none = SyncMirrorEvent {
            vault_path: "/vault",
            index: 0,
            identity: "git|git@example.com:me/vault.git|main",
            backend: "git",
            status: "published",
            error: None,
        };
        let mirror_none = MirrorSyncMirror {
            vault_path: "/vault".to_string(),
            index: 0,
            identity: "git|git@example.com:me/vault.git|main".to_string(),
            backend: "git".to_string(),
            status: "published".to_string(),
            error: None,
        };
        assert_eq!(
            serde_json::to_value(&real_none).unwrap(),
            serde_json::to_value(&mirror_none).unwrap(),
            "SyncMirrorEvent mirror drifted from the emitted payload (None variant)"
        );
    }
}
