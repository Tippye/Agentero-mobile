use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq)]
enum InkIoFailure {
    BeforePublication,
    AfterRename,
}

#[derive(Debug)]
enum InkIoEvent {
    Attempt(PathBuf, bool),
    // The visible bytes are asserted only by the unix-gated post-rename tests;
    // on other targets the payload is carried but never read.
    Visible(PathBuf, #[cfg_attr(not(unix), allow(dead_code))] Vec<u8>),
    Directory(PathBuf, PathBuf),
    RemotePut(String),
}

struct InkIoTrace {
    root: PathBuf,
    failure: Option<(PathBuf, InkIoFailure)>,
    events: Vec<InkIoEvent>,
}

thread_local! {
    static INK_IO_TRACE: std::cell::RefCell<Option<Arc<Mutex<InkIoTrace>>>> = const { std::cell::RefCell::new(None) };
}

struct InkIoGuard;
impl Drop for InkIoGuard {
    fn drop(&mut self) {
        INK_IO_TRACE.with(|trace| trace.replace(None));
    }
}

fn trace_ink_io(
    vault: &Path,
    failure: Option<(PathBuf, InkIoFailure)>,
) -> (InkIoGuard, Arc<Mutex<InkIoTrace>>) {
    let trace = Arc::new(Mutex::new(InkIoTrace {
        root: vault.to_owned(),
        failure,
        events: Vec::new(),
    }));
    INK_IO_TRACE.with(|slot| {
        assert!(slot.replace(Some(Arc::clone(&trace))).is_none());
    });
    (InkIoGuard, trace)
}

/// Only test builds route through this target-scoped hook. Both callbacks use
/// the real atomic publisher and bounded directory loop unless explicitly failed.
pub(super) fn write_atomic_for_test(vault: &Path, rel: &str, bytes: &[u8]) -> Result<(), AppError> {
    let trace = INK_IO_TRACE.with(|slot| slot.borrow().as_ref().cloned());
    let Some(trace) = trace.filter(|trace| trace.lock().unwrap().root == vault) else {
        return publish_sync_file(vault, rel, bytes, crate::fs::atomic_write_with, |path| {
            flush_ink_directories(path, |dir| fs::File::open(dir)?.sync_all())
        });
    };
    publish_sync_file(
        vault,
        rel,
        bytes,
        |target, proposed, opts| {
            {
                let mut trace = trace.lock().unwrap();
                trace
                    .events
                    .push(InkIoEvent::Attempt(target.to_owned(), opts.sync_file));
                if trace.failure.as_ref().is_some_and(|(path, point)| {
                    path == target && *point == InkIoFailure::BeforePublication
                }) {
                    return Err(std::io::Error::other("injected file publication failure"));
                }
            }
            crate::fs::atomic_write_with(target, proposed, opts)?;
            let actual = fs::read(target)?;
            trace
                .lock()
                .unwrap()
                .events
                .push(InkIoEvent::Visible(target.to_owned(), actual));
            Ok(())
        },
        |target| {
            flush_ink_directories(target, |directory| {
                {
                    let mut trace = trace.lock().unwrap();
                    trace.events.push(InkIoEvent::Directory(
                        target.to_owned(),
                        directory.to_owned(),
                    ));
                    if trace.failure.as_ref().is_some_and(|(path, point)| {
                        path == target && *point == InkIoFailure::AfterRename
                    }) {
                        return Err(std::io::Error::other(
                            "injected post-rename directory fsync failure",
                        ));
                    }
                }
                fs::File::open(directory)?.sync_all()
            })
        },
    )
}

struct InkTraceStore<'a> {
    store: &'a MemStore,
    trace: Arc<Mutex<InkIoTrace>>,
}
impl RemoteStore for InkTraceStore<'_> {
    async fn ensure_root(&self) -> Result<(), AppError> {
        self.store.ensure_root().await
    }
    async fn probe_conditional_writes(&self) -> Result<bool, AppError> {
        self.store.probe_conditional_writes().await
    }
    async fn get(&self, key: &str) -> Result<Option<(Vec<u8>, String)>, AppError> {
        self.store.get(key).await
    }
    async fn put(
        &self,
        key: &str,
        body: Vec<u8>,
        condition: PutCondition,
    ) -> Result<PutOutcome, AppError> {
        self.trace
            .lock()
            .unwrap()
            .events
            .push(InkIoEvent::RemotePut(key.to_owned()));
        self.store.put(key, body, condition).await
    }
}

fn native_stroke(
    vault: &Path,
    id: u128,
    actor: u128,
) -> crate::features::pdf::ink_store::InkPageSnapshot {
    use crate::features::pdf::ink_store::{
        apply_drafts, read_page, InkDraftPoint, InkEdit, InkStrokeDraft,
    };
    let baseline = read_page(vault, "papers/x", 1).unwrap();
    apply_drafts(
        vault,
        "papers/x",
        1,
        baseline.revision.as_deref(),
        uuid::Uuid::from_u128(actor),
        100,
        &[InkEdit::Put {
            stroke: InkStrokeDraft {
                id: uuid::Uuid::from_u128(id),
                tool: crate::features::pdf::ink::InkTool::Pen,
                color: [actor as u8, 20, 30, 255],
                base_width: 1.5,
                points: vec![InkDraftPoint {
                    x: 10.,
                    y: 20.,
                    pressure: Some(0.5),
                    elapsed_ms: Some(0),
                    tilt: None,
                }],
                created_at_ms: None,
                updated_at_ms: None,
            },
        }],
    )
    .unwrap()
}

async fn acknowledged_ink_pair(
    same_id: bool,
) -> (
    MemStore,
    SyncBackendConfig,
    tempfile::TempDir,
    tempfile::TempDir,
    crate::features::pdf::ink::InkPage,
) {
    let store = MemStore::default();
    let cfg = SyncBackendConfig::default();
    let (a, b) = mark_vault_pair(&store, INK_PATH, &ink_bytes(Vec::new(), Vec::new())).await;
    let acknowledged = native_stroke(a.path(), 1, 10).document;
    native_stroke(b.path(), if same_id { 1 } else { 2 }, 20);
    run_sync(b.path(), &cfg, &store, noop_progress())
        .await
        .unwrap();
    let local_hash = snapshot::hash_file(&a.path().join(INK_PATH)).unwrap();
    assert!(
        !store
            .objects
            .lock()
            .unwrap()
            .contains_key(&blob_key(&local_hash)),
        "acknowledged A is still only local"
    );
    (store, cfg, a, b, acknowledged)
}

fn assert_no_staging_files(vault: &Path) {
    assert!(fs::read_dir(vault.join("papers/x/marks/ink"))
        .unwrap()
        .all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")
        }));
}

#[tokio::test]
async fn ink_publication_failure_keeps_acknowledged_original_and_stops_before_upload() {
    let (store, cfg, a, _b, _) = acknowledged_ink_pair(false).await;
    let target = a.path().join(INK_PATH);
    let original = fs::read(&target).unwrap();
    let base = local::read_base(a.path()).unwrap().files[INK_PATH].hash.clone();
    let head = store.get(HEAD_KEY).await.unwrap().unwrap().0;
    let before = store.counts().1;
    let (_guard, trace) = trace_ink_io(
        a.path(),
        Some((target.clone(), InkIoFailure::BeforePublication)),
    );
    let error = run_sync(a.path(), &cfg, &store, noop_progress())
        .await
        .unwrap_err();
    assert_eq!(error.code(), "io");
    assert_eq!(fs::read(&target).unwrap(), original);
    assert_eq!(local::read_base(a.path()).unwrap().files[INK_PATH].hash, base);
    assert_eq!(store.get(HEAD_KEY).await.unwrap().unwrap().0, head);
    assert_eq!(store.counts().1, before);
    let trace = trace.lock().unwrap();
    assert!(trace
        .events
        .iter()
        .any(|event| matches!(event, InkIoEvent::Attempt(path, true) if path == &target)));
    assert!(!trace
        .events
        .iter()
        .any(|event| matches!(event, InkIoEvent::Visible(_, _))));
    assert_no_staging_files(a.path());
}

#[cfg(unix)]
#[tokio::test]
async fn ink_postrename_failure_keeps_valid_acknowledged_merge_and_aborts_remote_publication() {
    use crate::features::pdf::ink_store::read_page;
    let (store, cfg, a, b, acknowledged) = acknowledged_ink_pair(false).await;
    let target = a.path().join(INK_PATH);
    let original = fs::read(&target).unwrap();
    let base = local::read_base(a.path()).unwrap().files[INK_PATH].hash.clone();
    let head = store.get(HEAD_KEY).await.unwrap().unwrap().0;
    let before = store.counts().1;
    let (guard, trace) = trace_ink_io(a.path(), Some((target.clone(), InkIoFailure::AfterRename)));
    let error = run_sync(a.path(), &cfg, &store, noop_progress())
        .await
        .unwrap_err();
    assert_eq!(error.code(), "sync_ink_durability_uncertain");
    assert!(error.to_string().contains("may already be visible"));
    let actual = fs::read(&target).unwrap();
    assert_ne!(actual, original);
    let page = read_page(a.path(), "papers/x", 1).unwrap().document;
    assert_eq!(page.strokes.len(), 2);
    assert_eq!(page.strokes[0], acknowledged.strokes[0]);
    assert_eq!(local::read_base(a.path()).unwrap().files[INK_PATH].hash, base);
    assert_eq!(store.get(HEAD_KEY).await.unwrap().unwrap().0, head);
    assert_eq!(
        store.counts().1,
        before,
        "failure must stop before blob/manifest/HEAD PUT"
    );
    assert!(!store
        .objects
        .lock()
        .unwrap()
        .contains_key(&blob_key(&snapshot::hash_bytes(&actual))));
    assert!(trace.lock().unwrap().events.iter().any(
        |event| matches!(event, InkIoEvent::Visible(path, raw) if path == &target && raw == &actual)
    ));
    assert_no_staging_files(a.path());
    drop(guard);
    run_sync(a.path(), &cfg, &store, noop_progress())
        .await
        .unwrap();
    run_sync(b.path(), &cfg, &store, noop_progress())
        .await
        .unwrap();
    assert_eq!(
        read_page(b.path(), "papers/x", 1).unwrap().document,
        page,
        "retry reconciles the same IDs without rollback"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn ink_raw_conflict_postrename_failure_retains_losing_bytes_without_upload_or_rollback() {
    let (store, cfg, a, _b, _) = acknowledged_ink_pair(true).await;
    let original = fs::read(a.path().join(INK_PATH)).unwrap();
    let copy = a.path().join(mark_conflict_name(
        INK_PATH,
        &snapshot::hash_bytes(&original),
    ));
    let base = local::read_base(a.path()).unwrap().files[INK_PATH].hash.clone();
    let head = store.get(HEAD_KEY).await.unwrap().unwrap().0;
    let before = store.counts().1;
    let (guard, trace) = trace_ink_io(a.path(), Some((copy.clone(), InkIoFailure::AfterRename)));
    let error = run_sync(a.path(), &cfg, &store, noop_progress())
        .await
        .unwrap_err();
    assert_eq!(error.code(), "sync_ink_durability_uncertain");
    assert_eq!(fs::read(&copy).unwrap(), original);
    assert_eq!(
        fs::read(a.path().join(INK_PATH)).unwrap(),
        original,
        "conflict sources publish before replacing the acknowledged canonical page"
    );
    assert_eq!(local::read_base(a.path()).unwrap().files[INK_PATH].hash, base);
    assert_eq!(store.get(HEAD_KEY).await.unwrap().unwrap().0, head);
    assert_eq!(store.counts().1, before);
    {
        let trace = trace.lock().unwrap();
        assert!(trace
            .events
            .iter()
            .any(|event| matches!(event, InkIoEvent::Attempt(path, true) if path == &copy)));
        assert!(trace.events.iter().any(|event| matches!(event, InkIoEvent::Visible(path, bytes) if path == &copy && bytes == &original)));
        assert_no_staging_files(a.path());
    }
    drop(guard);

    // Presence after an uncertain rename is not proof of durability. Retry
    // must flush that same raw sibling before discarding canonical A.
    let (_retry_guard, retry_trace) = trace_ink_io(a.path(), None);
    let traced = InkTraceStore {
        store: &store,
        trace: Arc::clone(&retry_trace),
    };
    run_sync(a.path(), &cfg, &traced, noop_progress())
        .await
        .unwrap();
    assert_eq!(fs::read(&copy).unwrap(), original);
    assert!(store
        .objects
        .lock()
        .unwrap()
        .contains_key(&blob_key(&snapshot::hash_bytes(&original))));
    let trace = retry_trace.lock().unwrap();
    let canonical = a.path().join(INK_PATH);
    let replaced = trace
        .events
        .iter()
        .position(|event| matches!(event, InkIoEvent::Visible(path, _) if path == &canonical))
        .unwrap();
    let flushed: Vec<_> = trace
        .events
        .iter()
        .enumerate()
        .filter_map(|(index, event)| match event {
            InkIoEvent::Directory(path, _) if path == &copy => Some(index),
            _ => None,
        })
        .collect();
    assert_eq!(flushed.len(), 3);
    assert!(flushed.iter().all(|index| *index < replaced));
}

#[tokio::test]
async fn ink_canonical_download_and_conflict_sources_are_flushed_before_any_upload() {
    let (store, cfg, a, _b, acknowledged) = acknowledged_ink_pair(true).await;
    let original = fs::read(a.path().join(INK_PATH)).unwrap();
    let losing_copy = a.path().join(mark_conflict_name(
        INK_PATH,
        &snapshot::hash_bytes(&original),
    ));
    let (guard, trace) = trace_ink_io(a.path(), None);
    let traced = InkTraceStore {
        store: &store,
        trace: Arc::clone(&trace),
    };
    let result = run_sync(a.path(), &cfg, &traced, noop_progress())
        .await
        .unwrap();
    assert_eq!(result.conflict_copies.len(), 2);
    assert_eq!(fs::read(&losing_copy).unwrap(), original);
    let winner = crate::features::pdf::ink_store::read_page(a.path(), "papers/x", 1)
        .unwrap()
        .document;
    assert_ne!(
        winner.strokes[0], acknowledged.strokes[0],
        "losing A now survives only in its exact conflict sibling"
    );
    {
        let trace = trace.lock().unwrap();
        let first_upload = trace
            .events
            .iter()
            .position(|event| matches!(event, InkIoEvent::RemotePut(_)))
            .unwrap();
        assert!(trace
            .events
            .iter()
            .any(|event| matches!(event, InkIoEvent::RemotePut(key) if key == HEAD_KEY)));
        for path in result
            .conflict_copies
            .iter()
            .map(|path| a.path().join(path))
            .chain(std::iter::once(a.path().join(INK_PATH)))
        {
            let visible = trace
                .events
                .iter()
                .position(
                    |event| matches!(event, InkIoEvent::Visible(target, _) if target == &path),
                )
                .unwrap();
            assert!(trace.events.iter().any(
                |event| matches!(event, InkIoEvent::Attempt(target, true) if target == &path)
            ));
            let flushed: Vec<_> = trace
                .events
                .iter()
                .enumerate()
                .filter_map(|(i, event)| match event {
                    InkIoEvent::Directory(target, directory) if target == &path => {
                        Some((i, directory.clone()))
                    }
                    _ => None,
                })
                .collect();
            let expected: Vec<_> = path
                .ancestors()
                .skip(1)
                .take(if cfg!(unix) { 3 } else { 0 })
                .map(Path::to_owned)
                .collect();
            assert_eq!(
                flushed
                    .iter()
                    .map(|(_, path)| path.clone())
                    .collect::<Vec<_>>(),
                expected
            );
            assert!(flushed
                .iter()
                .all(|(index, _)| visible < *index && *index < first_upload));
        }
    }
    drop(guard);

    // A first-time peer has no ink base or file, so this is plan.downloads,
    // while the merge/conflict case above exercised plan.writes.
    let c = tempfile::tempdir().unwrap();
    let (_guard, trace) = trace_ink_io(c.path(), None);
    run_sync(c.path(), &cfg, &store, noop_progress())
        .await
        .unwrap();
    let target = c.path().join(INK_PATH);
    assert!(trace
        .lock()
        .unwrap()
        .events
        .iter()
        .any(|event| matches!(event, InkIoEvent::Attempt(path, true) if path == &target)));
    assert_eq!(
        fs::read(target).unwrap(),
        fs::read(a.path().join(INK_PATH)).unwrap()
    );
}

#[test]
fn ink_durability_classifier_does_not_flush_unrelated_json() {
    let vault = tempfile::tempdir().unwrap();
    let (_guard, trace) = trace_ink_io(vault.path(), None);
    for path in [
        ANNOTATION_PATH,
        "papers/x/marks/a.json",
        "notes/p1.json",
        "papers/x/marks/ink/p0.json",
        "papers/x/marks/ink/p1 (conflict not-a-sha).json",
    ] {
        crate::fs::with_file_mutation(|| write_atomic(vault.path(), path, b"{}")).unwrap();
    }
    let trace = trace.lock().unwrap();
    assert!(trace.events.iter().all(|event| !matches!(
        event,
        InkIoEvent::Attempt(_, true) | InkIoEvent::Directory(_, _)
    )));
    let first = mark_conflict_name(INK_PATH, &"a".repeat(64));
    assert!(durable_ink_path(&first));
    assert!(durable_ink_path(&mark_conflict_name(
        &first,
        &"b".repeat(64)
    )));
}
