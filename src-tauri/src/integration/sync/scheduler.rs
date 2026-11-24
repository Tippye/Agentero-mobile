//! Automatic sync scheduling. Desktop watches files; mobile receives explicit
//! native write/lifecycle signals and schedules only while foregrounded.
use super::SyncService;
#[cfg(any(desktop, test))]
use crate::core::error::AppError;
use crate::integration::sync::config;
#[cfg(desktop)]
use notify_debouncer_full::notify::{RecursiveMode, Watcher};
#[cfg(desktop)]
use notify_debouncer_full::{new_debouncer, DebouncedEvent};
#[cfg(mobile)]
use std::sync::atomic::Ordering;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::Duration,
};
use tauri::{AppHandle, Manager};
#[cfg(any(mobile, test))]
use tokio::sync::watch;
#[cfg(desktop)]
use tokio::time::sleep_until;
use tokio::time::Instant;

const CHANGE_QUIET_SECS: u64 = 30;
#[cfg(desktop)]
const EXIT_FLUSH_TIMEOUT_SECS: u64 = 5;

/// Backoff between automatic retries of a transport failure. A NAS whose DDNS
/// name briefly stops resolving (or that reboots) recovers long before the
/// regular interval; the last step caps retry frequency for a long outage.
/// Credential, configuration and HTTP-status failures are not retried.
#[cfg(desktop)]
const RETRY_DELAYS: &[Duration] = &[
    Duration::from_secs(30),
    Duration::from_secs(2 * 60),
    Duration::from_secs(10 * 60),
];

pub(super) struct ScheduledVault {
    task: tauri::async_runtime::JoinHandle<()>,
    #[cfg(mobile)]
    control: watch::Sender<MobileSignals>,
}
pub(super) type SchedulerMap = HashMap<String, ScheduledVault>;

fn stop_task(scheduled: ScheduledVault) {
    #[cfg(desktop)]
    scheduled.task.abort();
    #[cfg(mobile)]
    {
        // Detach after signalling stop. An admitted pass retains its lease until completion.
        scheduled.control.send_modify(|state| state.enabled = false);
        drop(scheduled.task);
    }
}

impl SyncService {
    /// Restart according to current metadata. Every pass resolves credentials anew.
    pub fn restart_scheduler(&self, app: &AppHandle, vault_key: &str) {
        let cfg = match config::try_get(vault_key) {
            Ok(cfg) => cfg,
            Err(_) => {
                log::warn!(target: "agentero::sync", "auto-sync configuration unavailable for {vault_key}");
                self.stop_scheduler(vault_key);
                return;
            }
        };
        let mut schedulers = self
            .schedulers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(old) = schedulers.remove(vault_key) {
            stop_task(old);
        }
        let Some(cfg) = cfg.filter(|cfg| cfg.auto_sync()) else {
            return;
        };
        let dir = PathBuf::from(vault_key);
        if !dir.is_dir() {
            return;
        }
        #[cfg(desktop)]
        let scheduled = ScheduledVault {
            task: tauri::async_runtime::spawn(run_desktop_loop(
                app.clone(),
                vault_key.to_owned(),
                dir,
                cfg.interval_minutes(),
            )),
        };
        #[cfg(mobile)]
        let scheduled = {
            let (control, receiver) =
                watch::channel(MobileSignals::new(self.foreground.load(Ordering::SeqCst)));
            let app = app.clone();
            let key = vault_key.to_owned();
            let task = tauri::async_runtime::spawn(run_mobile_schedule(
                receiver,
                interval_duration(cfg.interval_minutes()),
                move || {
                    let app = app.clone();
                    let key = key.clone();
                    async move {
                        let service = app.state::<SyncService>();
                        if !service.foreground.load(Ordering::SeqCst) {
                            return PassControl::Retry;
                        }
                        match config::try_get(&key) {
                            Ok(Some(cfg)) if cfg.auto_sync() => {}
                            Ok(_) => return PassControl::Stop,
                            Err(_) => {
                                log::warn!(target: "agentero::sync", "auto-sync configuration unavailable for {key}");
                                return PassControl::Retry;
                            }
                        }
                        if service.is_running(&key) {
                            return PassControl::Retry;
                        }
                        // A paused ask-policy decision blocks automatic retries:
                        // the user has not chosen yet, so a pass would only
                        // re-detect the same conflicts and rewrite the prompt.
                        if super::commands::has_pending_conflicts(&app, &key) {
                            return PassControl::Retry;
                        }
                        if let Err(error) =
                            super::commands::perform_sync(&app, &service, &key).await
                        {
                            log::warn!(target: "agentero::sync", "auto sync {key}: {error}");
                            if service.is_running(&key) {
                                return PassControl::Retry;
                            }
                        }
                        PassControl::Completed
                    }
                },
            ));
            ScheduledVault { task, control }
        };
        schedulers.insert(vault_key.to_owned(), scheduled);
    }

    pub fn stop_scheduler(&self, vault_key: &str) {
        let scheduled = self
            .schedulers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(vault_key);
        if let Some(scheduled) = scheduled {
            stop_task(scheduled);
        }
    }

    /// Start existing auto-sync bindings at application startup.
    pub fn start_all(&self, app: &AppHandle) {
        let configs = match config::try_list_all() {
            Ok(configs) => configs,
            Err(_) => {
                log::warn!(target: "agentero::sync", "auto-sync configuration catalog unavailable");
                return;
            }
        };
        for key in configs.keys() {
            self.restart_scheduler(app, key);
        }
    }

    /// Notify after a successful native local write, including deletes/moves.
    /// Desktop retains its file watcher and does not schedule an extra pass.
    pub fn notify_path_changed(&self, app: &AppHandle, path: &Path) {
        #[cfg(desktop)]
        let _ = (app, path);
        #[cfg(mobile)]
        {
            let configs = match config::try_list_all() {
                Ok(configs) => configs,
                Err(_) => {
                    log::warn!(target: "agentero::sync", "local-change sync configuration unavailable");
                    return;
                }
            };
            for (key, cfg) in configs {
                if !cfg.auto_sync() || !relevant_to_vault(Path::new(&key), path) {
                    continue;
                }
                let exists = self
                    .schedulers
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .contains_key(&key);
                if !exists {
                    self.restart_scheduler(app, &key);
                }
                if let Some(scheduled) = self
                    .schedulers
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .get(&key)
                {
                    scheduled.control.send_modify(|state| {
                        state.change_serial = state.change_serial.wrapping_add(1);
                        state.changed_at = Some(Instant::now());
                    });
                }
            }
        }
    }

    /// Native lifecycle gate. Backgrounding never cancels an in-flight pass.
    pub fn set_foreground(&self, app: &AppHandle, foreground: bool) {
        #[cfg(desktop)]
        let _ = (app, foreground);
        #[cfg(mobile)]
        {
            let previous = self.foreground.swap(foreground, Ordering::SeqCst);
            if previous == foreground {
                return;
            }
            {
                let schedulers = self
                    .schedulers
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                for scheduled in schedulers.values() {
                    scheduled.control.send_modify(|state| {
                        state.foreground = foreground;
                        if foreground {
                            state.resume_serial = state.resume_serial.wrapping_add(1);
                        }
                    });
                }
            }
            if foreground {
                // A configuration may have been added while no worker existed.
                let Ok(configs) = config::try_list_all() else {
                    return;
                };
                for (key, cfg) in configs {
                    if !cfg.auto_sync() {
                        continue;
                    }
                    let exists = self
                        .schedulers
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .contains_key(&key);
                    if !exists {
                        self.restart_scheduler(app, &key);
                    }
                }
            }
        }
    }

    /// Desktop-only bounded exit flush, with credential references resolved by Rust.
    #[cfg(desktop)]
    pub fn flush_on_exit(&self, app: &AppHandle) {
        let Ok(configs) = config::try_list_all() else {
            return;
        };
        let token = tauri::async_runtime::block_on(super::commands::session_bearer(app));
        for (key, metadata) in configs {
            if !metadata.auto_sync() || metadata.primary.backend == config::SyncBackendKind::Git {
                continue;
            }
            let dir = PathBuf::from(&key);
            if !dir.is_dir() {
                continue;
            }
            let Some(_lease) = self.try_begin(&key) else {
                log::info!(target: "agentero::sync", "exit flush {key}: sync already running; skipped");
                continue;
            };
            let Some(_lock) = agentero_core::features::sync::lock::SyncLock::try_acquire(&dir)
                .ok()
                .flatten()
            else {
                log::info!(target: "agentero::sync", "exit flush {key}: vault lock busy; skipped");
                continue;
            };
            let resolved = tauri::async_runtime::block_on(super::credentials::load(app, &key));
            let mut cfg = match resolved {
                Ok(Some(cfg))
                    if cfg.auto_sync() && cfg.primary.backend != config::SyncBackendKind::Git =>
                {
                    cfg
                }
                Ok(_) => continue,
                Err(_) => {
                    log::warn!(target: "agentero::sync", "exit flush {key}: credentials unavailable; skipped");
                    continue;
                }
            };
            if cfg.primary.backend == config::SyncBackendKind::Agentero {
                if cfg.primary.agentero_api.trim().is_empty() {
                    cfg.primary.agentero_api = super::commands::account_api_origin();
                }
                if let Some(token) = token.clone() {
                    cfg.primary.agentero_token = token;
                }
            }
            let result = tauri::async_runtime::block_on(async {
                tokio::time::timeout(
                    Duration::from_secs(EXIT_FLUSH_TIMEOUT_SECS),
                    super::engine::sync_vault_locked(&_lock, &cfg.primary, &|_, _, _| {}),
                )
                .await
            });
            match result {
                Ok(Ok(outcome)) => {
                    log::info!(target: "agentero::sync", "exit flush {key}: version={} up={} down={}", outcome.version, outcome.uploaded, outcome.downloaded)
                }
                Ok(Err(error)) => log::warn!(target: "agentero::sync", "exit flush {key}: {error}"),
                Err(_) => log::warn!(target: "agentero::sync", "exit flush {key}: timed out"),
            }
        }
    }
}

fn interval_duration(minutes: u32) -> Duration {
    Duration::from_secs(u64::from(minutes.max(1)) * 60)
}

#[cfg(desktop)]
async fn run_desktop_loop(app: AppHandle, vault_key: String, dir: PathBuf, interval_minutes: u32) {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<()>(1);
    let mut debouncer = match new_debouncer(
        Duration::from_secs(CHANGE_QUIET_SECS),
        None,
        move |events: Result<Vec<DebouncedEvent>, Vec<notify_debouncer_full::notify::Error>>| {
            let relevant = events
                .map(|batch| {
                    batch
                        .iter()
                        .any(|event| event.event.paths.iter().any(|path| !is_ignored_path(path)))
                })
                .unwrap_or(false);
            if relevant {
                let _ = tx.try_send(());
            }
        },
    ) {
        Ok(debouncer) => Some(debouncer),
        Err(error) => {
            log::warn!(target: "agentero::sync", "auto-sync watcher init failed: {error}");
            None
        }
    };
    if let Some(debouncer) = &mut debouncer {
        if let Err(error) = debouncer.watcher().watch(&dir, RecursiveMode::Recursive) {
            log::warn!(target: "agentero::sync", "auto-sync watch failed: {error}");
        }
    }
    let mut ticker = tokio::time::interval(interval_duration(interval_minutes));
    let mut failures = 0usize;
    let mut retry_at: Option<Instant> = None;
    loop {
        // `select!` needs a concrete deadline; the guard disables the branch
        // while no retry is pending.
        let retry_deadline =
            retry_at.unwrap_or_else(|| Instant::now() + Duration::from_secs(86_400));
        tokio::select! {
            _ = ticker.tick() => {},
            changed = rx.recv() => { if changed.is_none() { break; } }
            _ = sleep_until(retry_deadline), if retry_at.is_some() => {}
        }
        // A paused ask-policy decision blocks automatic retries: the user has
        // not chosen yet, so a pass would only re-detect the same conflicts.
        if super::commands::has_pending_conflicts(&app, &vault_key) {
            failures = 0;
            retry_at = None;
            continue;
        }
        let outcome = {
            let service = app.state::<SyncService>();
            super::commands::perform_sync(&app, &service, &vault_key).await
        };
        match outcome {
            Ok(_) => {
                failures = 0;
                retry_at = None;
            }
            Err(error) => {
                log::warn!(target: "agentero::sync", "auto sync {vault_key}: {error}");
                match retry_delay(&error, failures) {
                    Some(delay) => {
                        retry_at = Some(Instant::now() + delay);
                        failures = failures.saturating_add(1);
                    }
                    None => {
                        // A failure that needs user action resets the ladder;
                        // a later outage starts from the first step again.
                        retry_at = None;
                        failures = 0;
                    }
                }
            }
        }
    }
}

/// Delay before re-attempting an automatic pass; `None` when the failure needs
/// user action and the regular interval should take over again.
#[cfg(desktop)]
fn retry_delay(error: &AppError, consecutive_failures: usize) -> Option<Duration> {
    if error.code() != agentero_core::features::sync::SYNC_UNREACHABLE {
        return None;
    }
    let index = consecutive_failures.min(RETRY_DELAYS.len() - 1);
    RETRY_DELAYS.get(index).copied()
}

fn is_ignored_path(path: &Path) -> bool {
    path.components()
        .any(|component| super::snapshot::is_ignored_name(&component.as_os_str().to_string_lossy()))
}
// Mobile local-change triggers use these; on desktop only the unix alias test
// exercises them, so other desktop targets must not flag them as dead code.
#[cfg(any(mobile, all(test, unix)))]
fn relevant_to_vault(vault: &Path, path: &Path) -> bool {
    // Match filesystem identity (/var and /private/var on iOS), while retaining
    // the original configuration key for the credential namespace and run lease.
    let Ok(vault) = vault.canonicalize() else {
        return false;
    };
    let Some(path) = canonical_changed_path(path) else {
        return false;
    };
    relevant_canonical_paths(&vault, &path)
}

#[cfg(any(mobile, all(test, unix)))]
fn canonical_changed_path(path: &Path) -> Option<PathBuf> {
    let mut cursor = path;
    let mut missing = Vec::new();
    loop {
        match cursor.canonicalize() {
            Ok(mut resolved) => {
                for component in missing.into_iter().rev() {
                    resolved.push(component);
                }
                return Some(resolved);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // Deleted paths cannot be canonicalized. Resolve the nearest
                // existing ancestor, then reattach only unambiguous normal names.
                if path
                    .components()
                    .any(|component| matches!(component, std::path::Component::ParentDir))
                {
                    return None;
                }
                missing.push(cursor.file_name()?.to_os_string());
                cursor = cursor.parent()?;
            }
            Err(_) => return None,
        }
    }
}

#[cfg(any(mobile, test))]
fn relevant_canonical_paths(vault: &Path, path: &Path) -> bool {
    path.strip_prefix(vault).is_ok_and(|relative| {
        !relative
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
            && !is_ignored_path(relative)
    })
}

#[cfg(any(mobile, test))]
#[derive(Clone, Copy, PartialEq, Eq)]
struct MobileSignals {
    enabled: bool,
    foreground: bool,
    resume_serial: u64,
    change_serial: u64,
    changed_at: Option<Instant>,
}
#[cfg(any(mobile, test))]
impl MobileSignals {
    fn new(foreground: bool) -> Self {
        Self {
            enabled: true,
            foreground,
            resume_serial: 0,
            change_serial: 0,
            changed_at: None,
        }
    }
}
#[cfg(any(mobile, test))]
#[derive(Clone, Copy)]
enum PassControl {
    Completed,
    Retry,
    Stop,
}

/// The admitted pass is deliberately awaited outside select: lifecycle changes
/// affect subsequent admission and never drop a sync future or its lease.
#[cfg(any(mobile, test))]
async fn run_mobile_schedule<F, Fut>(
    mut receiver: watch::Receiver<MobileSignals>,
    interval: Duration,
    mut pass: F,
) where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = PassControl>,
{
    let mut initial = true;
    let mut handled_resume = 0;
    let mut handled_change = 0;
    let mut next_interval = Instant::now() + interval;
    let mut retry_at = None;
    loop {
        let signals = *receiver.borrow_and_update();
        if !signals.enabled {
            return;
        }
        if !signals.foreground {
            if receiver.changed().await.is_err() {
                return;
            }
            continue;
        }
        let now = Instant::now();
        let mut due = next_interval;
        if initial || signals.resume_serial != handled_resume {
            due = now;
        }
        if signals.change_serial != handled_change {
            let quiet = signals.changed_at.unwrap_or(now) + Duration::from_secs(CHANGE_QUIET_SECS);
            due = due.min(quiet);
        }
        if let Some(retry) = retry_at {
            due = due.max(retry);
        }
        tokio::select! {
            biased;
            changed = receiver.changed() => { if changed.is_err() { return; } continue; },
            _ = tokio::time::sleep_until(due) => {}
        }
        // A newer change can extend the quiet period just as a timer becomes ready.
        if *receiver.borrow_and_update() != signals {
            continue;
        }
        match pass().await {
            PassControl::Stop => return,
            PassControl::Retry => retry_at = Some(Instant::now() + Duration::from_secs(1)),
            PassControl::Completed => {
                initial = false;
                handled_resume = signals.resume_serial;
                handled_change = signals.change_serial;
                next_interval = Instant::now() + interval;
                retry_at = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    async fn settle() {
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;
    }
    fn changed(sender: &watch::Sender<MobileSignals>) {
        sender.send_modify(|state| {
            state.change_serial += 1;
            state.changed_at = Some(Instant::now());
        });
    }
    fn spawn_counter(
        foreground: bool,
        interval: Duration,
    ) -> (
        watch::Sender<MobileSignals>,
        Arc<AtomicUsize>,
        tokio::task::JoinHandle<()>,
    ) {
        let (sender, receiver) = watch::channel(MobileSignals::new(foreground));
        let count = Arc::new(AtomicUsize::new(0));
        let passes = count.clone();
        let task = tokio::spawn(run_mobile_schedule(receiver, interval, move || {
            passes.fetch_add(1, Ordering::SeqCst);
            async { PassControl::Completed }
        }));
        (sender, count, task)
    }
    #[tokio::test(start_paused = true)]
    async fn local_changes_coalesce_for_thirty_quiet_seconds() {
        let (sender, count, task) = spawn_counter(true, Duration::from_secs(900));
        settle().await;
        assert_eq!(count.load(Ordering::SeqCst), 1);
        changed(&sender);
        settle().await;
        tokio::time::advance(Duration::from_secs(20)).await;
        changed(&sender);
        settle().await;
        tokio::time::advance(Duration::from_secs(29)).await;
        settle().await;
        assert_eq!(count.load(Ordering::SeqCst), 1);
        tokio::time::advance(Duration::from_secs(1)).await;
        settle().await;
        assert_eq!(count.load(Ordering::SeqCst), 2);
        sender.send_modify(|state| state.enabled = false);
        task.await.unwrap();
    }
    #[tokio::test(start_paused = true)]
    async fn background_defers_changes_and_intervals_until_one_resume_pass() {
        let (sender, count, task) = spawn_counter(false, Duration::from_secs(60));
        settle().await;
        changed(&sender);
        tokio::time::advance(Duration::from_secs(600)).await;
        settle().await;
        assert_eq!(count.load(Ordering::SeqCst), 0);
        sender.send_modify(|state| {
            state.foreground = true;
            state.resume_serial += 1;
        });
        settle().await;
        assert_eq!(count.load(Ordering::SeqCst), 1);
        tokio::time::advance(Duration::from_secs(59)).await;
        settle().await;
        assert_eq!(count.load(Ordering::SeqCst), 1);
        tokio::time::advance(Duration::from_secs(1)).await;
        settle().await;
        assert_eq!(count.load(Ordering::SeqCst), 2);
        sender.send_modify(|state| {
            state.foreground = false;
        });
        settle().await;
        tokio::time::advance(Duration::from_secs(600)).await;
        settle().await;
        assert_eq!(count.load(Ordering::SeqCst), 2);
        sender.send_modify(|state| state.enabled = false);
        task.await.unwrap();
    }
    #[tokio::test(start_paused = true)]
    async fn background_and_disable_do_not_abort_an_admitted_lease() {
        let (sender, receiver) = watch::channel(MobileSignals::new(true));
        let service = Arc::new(SyncService::default());
        let run_service = service.clone();
        let (release, released) = tokio::sync::oneshot::channel();
        let mut released = Some(released);
        let task = tokio::spawn(run_mobile_schedule(
            receiver,
            Duration::from_secs(60),
            move || {
                let service = run_service.clone();
                let released = released.take().unwrap();
                async move {
                    let _lease = service.try_begin("vault").unwrap();
                    released.await.unwrap();
                    PassControl::Completed
                }
            },
        ));
        settle().await;
        assert!(service.is_running("vault"));
        sender.send_modify(|state| state.foreground = false);
        settle().await;
        tokio::time::advance(Duration::from_secs(300)).await;
        settle().await;
        assert!(service.is_running("vault"));
        sender.send_modify(|state| state.enabled = false);
        settle().await;
        assert!(!task.is_finished());
        release.send(()).unwrap();
        task.await.unwrap();
        assert!(!service.is_running("vault"));
    }
    #[tokio::test(start_paused = true)]
    async fn changes_during_a_pass_are_not_lost_and_busy_leases_defer() {
        let (sender, receiver) = watch::channel(MobileSignals::new(true));
        let service = Arc::new(SyncService::default());
        let manual = service.try_begin("vault").unwrap();
        let passes = Arc::new(AtomicUsize::new(0));
        let count = passes.clone();
        let run_service = service.clone();
        let task = tokio::spawn(run_mobile_schedule(
            receiver,
            Duration::from_secs(900),
            move || {
                let service = run_service.clone();
                let count = count.clone();
                async move {
                    let Some(_lease) = service.try_begin("vault") else {
                        return PassControl::Retry;
                    };
                    count.fetch_add(1, Ordering::SeqCst);
                    PassControl::Completed
                }
            },
        ));
        settle().await;
        assert_eq!(passes.load(Ordering::SeqCst), 0);
        changed(&sender);
        drop(manual);
        tokio::time::advance(Duration::from_secs(1)).await;
        settle().await;
        assert_eq!(passes.load(Ordering::SeqCst), 1);
        // Even the same monotonic timestamp is a distinct write notification.
        changed(&sender);
        settle().await;
        tokio::time::advance(Duration::from_secs(30)).await;
        settle().await;
        assert_eq!(passes.load(Ordering::SeqCst), 2);
        sender.send_modify(|state| state.enabled = false);
        task.await.unwrap();
    }
    #[tokio::test(start_paused = true)]
    async fn disabled_workers_and_removed_controls_never_start_a_pass() {
        let mut state = MobileSignals::new(true);
        state.enabled = false;
        let (sender, receiver) = watch::channel(state);
        run_mobile_schedule(receiver, Duration::from_secs(60), || async {
            panic!("disabled scheduler dispatched")
        })
        .await;
        drop(sender);
        let (sender, receiver) = watch::channel(MobileSignals::new(false));
        drop(sender);
        run_mobile_schedule(receiver, Duration::from_secs(60), || async {
            PassControl::Stop
        })
        .await;
    }
    #[cfg(unix)]
    #[test]
    fn aliased_roots_match_existing_and_deleted_paths_without_crossing_vaults() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("vault");
        let alias = fixture.path().join("vault-alias");
        let outside = fixture.path().join("vault-other");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(root.join("notes.md"), "note").unwrap();
        std::os::unix::fs::symlink(&root, &alias).unwrap();
        let canonical_root = root.canonicalize().unwrap();
        let canonical_file = root.join("notes.md").canonicalize().unwrap();
        assert!(relevant_to_vault(&alias, &canonical_file));
        assert!(relevant_to_vault(&canonical_root, &alias.join("notes.md")));
        // Missing suffixes represent a delete/move event after the file or directory is gone.
        assert!(relevant_to_vault(
            &alias,
            &canonical_root.join("removed/notes.md")
        ));
        assert!(relevant_to_vault(
            &canonical_root,
            &alias.join("removed/notes.md")
        ));
        assert!(!relevant_to_vault(&alias, &outside.join("notes.md")));
        assert!(!relevant_to_vault(
            &alias,
            &alias.join(".agentero/sync/base.json")
        ));
        std::os::unix::fs::symlink(&outside, root.join("outside-link")).unwrap();
        assert!(!relevant_to_vault(
            &alias,
            &alias.join("outside-link/removed.md")
        ));
    }

    #[cfg(desktop)]
    #[test]
    fn automatic_retries_only_transport_failures_with_bounded_backoff() {
        use agentero_core::features::sync::SYNC_UNREACHABLE;

        let unreachable = AppError::domain(SYNC_UNREACHABLE, "dns error");
        assert_eq!(retry_delay(&unreachable, 0), Some(Duration::from_secs(30)));
        assert_eq!(retry_delay(&unreachable, 1), Some(Duration::from_secs(120)));
        assert_eq!(retry_delay(&unreachable, 2), Some(Duration::from_secs(600)));
        // A long outage keeps the last step instead of growing without bound.
        assert_eq!(retry_delay(&unreachable, 9), Some(Duration::from_secs(600)));

        // Everything that needs user action waits for the regular interval.
        assert_eq!(
            retry_delay(&AppError::domain("sync_credentials_missing", "nope"), 0),
            None
        );
        assert_eq!(
            retry_delay(&AppError::message("sync is not configured"), 3),
            None
        );
        assert_eq!(
            retry_delay(
                &AppError::domain("sync_backend_unavailable_mobile", "nope"),
                1
            ),
            None
        );
    }

    #[test]
    fn local_trigger_is_vault_scoped_and_ignores_sync_and_vcs_state() {
        let root = Path::new("/private/vault");
        assert!(relevant_canonical_paths(
            root,
            Path::new("/private/vault/papers/p1/notes.md")
        ));
        assert!(!relevant_canonical_paths(
            root,
            Path::new("/private/vault-other/notes.md")
        ));
        assert!(!relevant_canonical_paths(
            root,
            Path::new("/private/vault/.agentero/sync/base.json")
        ));
        assert!(!relevant_canonical_paths(
            root,
            Path::new("/private/vault/.git/index")
        ));
        assert!(!relevant_canonical_paths(
            root,
            Path::new("/private/vault/../outside.md")
        ));
        assert_eq!(
            interval_duration(u32::MAX).as_secs(),
            u64::from(u32::MAX) * 60
        );
    }
}
