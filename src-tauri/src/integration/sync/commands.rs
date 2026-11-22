//! Sync commands. Long-running `sync_now` broadcasts `sync:state` /
//! `sync:progress` events so the settings pane (and later a status bar
//! indicator) can follow along from any window.

use super::config::SyncBackendConfig;
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
use tauri::{AppHandle, Emitter, Manager, State};

#[derive(Debug, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SyncVaultArgs {
    pub vault_path: String,
}

#[derive(Debug, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SyncConfigureArgs {
    pub vault_path: String,
    pub config: SyncBackendConfig,
}

#[derive(Debug, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    pub configured: bool,
    pub credential_state: super::credentials::SyncCredentialState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<SyncBackendConfig>,
    pub running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_sync_at: Option<String>,
    pub last_version: u64,
    /// Divergent edits awaiting confirmation under the ask policy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_conflicts: Option<PendingConflicts>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SyncStateEvent<'a> {
    vault_path: &'a str,
    status: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SyncProgressEvent<'a> {
    vault_path: &'a str,
    phase: &'a str,
    current: usize,
    total: usize,
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
        let dir = vault_dir(&app, &args.vault_path)?;
        let status = super::credentials::status(&app, args.vault_path.trim()).await?;
        let meta = local::read_meta(&dir)?;
        let pending_conflicts = if status
            .config
            .as_ref()
            .is_some_and(|cfg| asks_for_conflicts(&dir, cfg))
        {
            conflicts::read_pending(&dir)?
        } else {
            None
        };
        Ok::<_, AppError>(SyncStatus {
            configured: status.config.is_some(),
            config: status.config,
            credential_state: status.state,
            running: service.is_running(args.vault_path.trim()),
            last_sync_at: meta.last_sync_at,
            last_version: meta.last_version,
            pending_conflicts,
        })
    };
    Ok(match inner.await {
        Ok(status) => ApiResult::ok(status),
        Err(error) => map_err(error),
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
        let _lock = SyncLock::try_acquire(&dir)?
            .ok_or_else(|| AppError::message("another vault sync is running"))?;
        let previous = super::config::try_get(&key)?;
        let mut candidate = args.config.normalized();
        attach_agentero_session(&app, &mut candidate).await;
        let configured = super::credentials::configure(&app, &key, &dir, candidate).await?;
        let config = &configured.config;
        let keep_review = asks_for_conflicts(&dir, config)
            && previous.as_ref().is_some_and(|old| {
                old.conflict_policy == config.conflict_policy
                    && old.scope == config.scope
                    && old.remote_identity() == config.remote_identity()
            });
        let pending_conflicts = if keep_review {
            conflicts::read_pending(&dir)?
        } else {
            conflicts::clear_locked(&_lock)?;
            None
        };
        let meta = local::read_meta(&dir)?;
        Ok::<_, AppError>(SyncStatus {
            configured: true,
            config: Some(configured.config.masked()),
            credential_state: if app
                .state::<Option<crate::integration::credentials::CredentialStore>>()
                .is_some()
            {
                super::credentials::SyncCredentialState::Ready
            } else {
                super::credentials::SyncCredentialState::Inline
            },
            running: false,
            last_sync_at: meta.last_sync_at,
            last_version: meta.last_version,
            pending_conflicts,
        })
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
        let _lock = SyncLock::try_acquire(&dir)?
            .ok_or_else(|| AppError::message("another vault sync is running"))?;
        super::credentials::clear(&app, &key).await?;
        local::clear(&_lock)?;
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
    let mut cfg = super::credentials::load(app, vault_key)
        .await?
        .ok_or_else(|| AppError::message("sync is not configured"))?;
    attach_agentero_session(app, &mut cfg).await;
    emit_state(app, vault_key, "syncing", None);
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
    let reason = if matches!(task, SyncTask::FetchPaper(_)) {
        crate::features::mobile_background::BackgroundReason::Transfer
    } else {
        crate::features::mobile_background::BackgroundReason::Sync
    };
    let result = crate::features::mobile_background::with_work(app, reason, async {
        match task {
            SyncTask::FetchPaper(paper) => {
                availability::fetch_paper_locked(&lock, &cfg, paper, &progress).await
            }
            SyncTask::Sync => engine::sync_vault_locked(&lock, &cfg, &progress).await,
            SyncTask::Resolve(args) => {
                engine::resolve_conflicts_locked(
                    &lock,
                    &cfg,
                    &args.review_id,
                    args.default_resolution,
                    args.resolutions,
                    &progress,
                )
                .await
            }
        }
    })
    .await;
    // Terminal event consumers immediately re-read status; release first.
    drop(lock);
    drop(lease);
    match result {
        Ok(outcome) => {
            emit_state(app, vault_key, "idle", None);
            Ok(outcome)
        }
        Err(e) => {
            if e.code() == conflicts::PENDING_CODE {
                emit_state(app, vault_key, "conflict", None);
            } else {
                emit_state(app, vault_key, "error", Some(e.to_string()));
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
        .is_some_and(|cfg| asks_for_conflicts(&dir, cfg))
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

fn emit_state(app: &AppHandle, vault_path: &str, status: &str, error: Option<String>) {
    let _ = app.emit(
        "sync:state",
        SyncStateEvent {
            vault_path,
            status,
            error,
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
    use super::{SyncProgressEvent, SyncStateEvent};
    use crate::app::events_contract::{
        SyncProgressEvent as MirrorSyncProgress, SyncStateEvent as MirrorSyncState,
    };

    #[test]
    fn sync_state_mirror_matches_emit_payload_shape() {
        let real = SyncStateEvent {
            vault_path: "/vault",
            status: "configured",
            error: Some("offline".to_string()),
        };
        let mirror = MirrorSyncState {
            vault_path: "/vault".to_string(),
            status: "configured".to_string(),
            error: Some("offline".to_string()),
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
        };
        let mirror_none = MirrorSyncState {
            vault_path: "/vault".to_string(),
            status: "disconnected".to_string(),
            error: None,
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
}
