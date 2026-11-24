use super::*;
use crate::features::mobile_background::{
    BackgroundCounts, BackgroundDriver, BackgroundPlatform, BackgroundSettingsTarget,
    BackgroundState,
};
use crate::features::remote_agent::{BackendId, HostRef, SessionInfo, WorkspaceRef};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::sync::oneshot;

#[derive(Default)]
struct Driver {
    counts: Mutex<Vec<BackgroundCounts>>,
    changed: Notify,
}
#[async_trait]
impl BackgroundDriver for Driver {
    async fn state(&self) -> Result<BackgroundState, AppError> {
        Ok(BackgroundState::unsupported(BackgroundPlatform::Desktop))
    }
    async fn apply(&self, counts: BackgroundCounts) -> Result<BackgroundState, AppError> {
        locked(&self.counts).push(counts);
        self.changed.notify_one();
        self.state().await
    }
    async fn configure(&self, _: bool) -> Result<BackgroundState, AppError> {
        self.state().await
    }
    async fn request_notifications(&self) -> Result<BackgroundState, AppError> {
        panic!("observer must never prompt for permission")
    }
    async fn open_settings(
        &self,
        _: BackgroundSettingsTarget,
    ) -> Result<BackgroundState, AppError> {
        panic!("observer must never open settings")
    }
}
impl Driver {
    async fn agents(&self, expected: u32) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if locked(&self.counts)
                    .last()
                    .is_some_and(|counts| counts.agent_count == expected)
                {
                    return;
                }
                self.changed.notified().await;
            }
        })
        .await
        .expect("native count did not settle");
    }
}

enum Step {
    Value(Result<Observation, ReadFailure>),
    Gate(oneshot::Receiver<Result<Observation, ReadFailure>>),
}
struct Source {
    steps: Mutex<VecDeque<Step>>,
    fallback: Mutex<Result<Observation, ReadFailure>>,
    calls: AtomicUsize,
    hint_calls: AtomicUsize,
    hint_steps: Mutex<VecDeque<Step>>,
    hint_fallback: Mutex<Result<Observation, ReadFailure>>,
    dropped: AtomicUsize,
    targets: Mutex<Vec<SessionRef>>,
    blocked: AtomicBool,
    changed: Notify,
}
impl Source {
    fn new() -> Self {
        Self {
            steps: Mutex::new(VecDeque::new()),
            fallback: Mutex::new(Ok(sample(TaskStatus::Running, 0, &[]))),
            calls: AtomicUsize::new(0),
            hint_calls: AtomicUsize::new(0),
            hint_steps: Mutex::new(VecDeque::new()),
            hint_fallback: Mutex::new(Err(ReadFailure::Unavailable)),
            dropped: AtomicUsize::new(0),
            targets: Mutex::new(Vec::new()),
            blocked: AtomicBool::new(false),
            changed: Notify::new(),
        }
    }
    fn push(&self, values: impl IntoIterator<Item = Result<Observation, ReadFailure>>) {
        locked(&self.steps).extend(values.into_iter().map(Step::Value));
    }
    async fn hint_calls(&self, count: usize) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while self.hint_calls.load(Ordering::SeqCst) < count {
                self.changed.notified().await;
            }
        })
        .await
        .expect("observer did not read the nonrotating hint path");
    }
    async fn calls(&self, count: usize) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while self.calls.load(Ordering::SeqCst) < count {
                self.changed.notified().await;
            }
        })
        .await
        .expect("observer did not read");
    }
}
#[async_trait]
impl ObservationSource for Source {
    async fn read(&self, session: &SessionRef) -> Result<Observation, ReadFailure> {
        locked(&self.targets).push(session.clone());
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.changed.notify_one();
        struct Reading<'a>(&'a Source);
        impl Drop for Reading<'_> {
            fn drop(&mut self) {
                self.0.dropped.fetch_add(1, Ordering::SeqCst);
                self.0.changed.notify_one();
            }
        }
        let _reading = Reading(self);
        if self.blocked.load(Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        let next = locked(&self.steps).pop_front();
        match next {
            Some(Step::Value(value)) => value,
            Some(Step::Gate(gate)) => gate.await.unwrap_or(Err(ReadFailure::Unavailable)),
            None => locked(&self.fallback).clone(),
        }
    }
    async fn read_network_hint(&self, session: &SessionRef) -> Result<Observation, ReadFailure> {
        locked(&self.targets).push(session.clone());
        self.hint_calls.fetch_add(1, Ordering::SeqCst);
        self.changed.notify_one();
        let step = locked(&self.hint_steps).pop_front();
        match step {
            Some(Step::Value(value)) => value,
            Some(Step::Gate(gate)) => gate.await.unwrap_or(Err(ReadFailure::Unavailable)),
            None => locked(&self.hint_fallback).clone(),
        }
    }
}
fn session() -> SessionRef {
    SessionRef {
        workspace: WorkspaceRef {
            host: HostRef {
                connection: ConnectionRef {
                    provider_id: DEFAULT_PROVIDER_ID.into(),
                    connection_id: "connection-a".into(),
                },
                host_id: "host-a".into(),
            },
            backend_id: "harness".into(),
            workspace_id: "workspace-a".into(),
        },
        session_id: "session-a".into(),
    }
}
fn request(session: SessionRef, serial: usize) -> OperationRequest {
    OperationRequest {
        operation_id: format!("background-operation-{serial:04}").as_str().into(),
        mutation: Mutation::SendMessage {
            session,
            text: "foreground-only prompt; never retained by monitor".into(),
            attachments: vec![],
        },
    }
}
fn point(sequence: i64) -> Point {
    Point {
        stream: "bound-session-stream".into(),
        sequence,
    }
}
fn sample(status: TaskStatus, through: i64, changes: &[(i64, TaskStatus)]) -> Observation {
    Observation {
        status,
        through: Some(point(through)),
        transitions: changes
            .iter()
            .map(|(sequence, status)| (point(*sequence), *status))
            .collect(),
    }
}
struct Fixture {
    driver: Arc<Driver>,
    source: Arc<Source>,
    common: Arc<MobileBackgroundService>,
    monitors: Arc<RemoteBackgroundMonitors>,
}
impl Fixture {
    fn new() -> Self {
        Self::with_policy(Policy {
            interval: Duration::from_millis(5),
            hint_interval: Duration::from_millis(5),
            baseline: Duration::from_millis(50),
            read: Duration::from_millis(30),
            failures: 3,
            settling: 4,
        })
    }
    fn with_policy(policy: Policy) -> Self {
        let driver = Arc::new(Driver::default());
        let common = MobileBackgroundService::new(driver.clone());
        let source = Arc::new(Source::new());
        let monitors = Arc::new(RemoteBackgroundMonitors::with_source(
            common.clone(),
            source.clone(),
            policy,
        ));
        monitors.select_host(&session().workspace.host);
        Self {
            driver,
            source,
            common,
            monitors,
        }
    }
    async fn prepare(&self, serial: usize) -> BackgroundTicket {
        self.monitors
            .prepare(&request(session(), serial))
            .await
            .unwrap()
            .unwrap()
    }
    async fn stopped(&self) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while !locked(&self.monitors.inner.registry).monitors.is_empty() {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("observer did not release registry");
        self.driver.agents(0).await;
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.monitors.shutdown();
    }
}

#[test]
fn unchanged_idle_after_ack_and_old_turn_end_after_queued_running_are_not_completion() {
    let mut progress = Progress::new(Some(point(5)));
    let idle = sample(
        TaskStatus::Idle,
        5,
        &[(1, TaskStatus::Running), (4, TaskStatus::Idle)],
    );
    assert_eq!(progress.observe(&idle, 6), ObservationResult::Continue);
    assert_eq!(progress.observe(&idle, 6), ObservationResult::Continue);
    let queued = sample(
        TaskStatus::Running,
        5,
        &[(1, TaskStatus::Running), (4, TaskStatus::Idle)],
    );
    assert_eq!(progress.observe(&queued, 6), ObservationResult::Continue);
    assert_eq!(progress.observe(&idle, 6), ObservationResult::Continue);
    assert_eq!(
        progress.observe(
            &sample(TaskStatus::Running, 6, &[(6, TaskStatus::Running)]),
            6
        ),
        ObservationResult::Continue
    );
    assert_eq!(
        progress.observe(
            &sample(
                TaskStatus::Idle,
                7,
                &[(6, TaskStatus::Running), (7, TaskStatus::Idle)]
            ),
            6
        ),
        ObservationResult::FreshTerminal
    );
}

#[test]
fn first_terminal_history_is_unproven_and_unknown_or_reset_cursors_never_claim_completion() {
    let done = sample(
        TaskStatus::Idle,
        9,
        &[(7, TaskStatus::Running), (9, TaskStatus::Idle)],
    );
    let mut progress = Progress::new(Some(point(0)));
    assert_eq!(progress.observe(&done, 3), ObservationResult::Continue);
    assert_eq!(progress.observe(&done, 3), ObservationResult::Continue);
    assert_eq!(
        progress.observe(&done, 3),
        ObservationResult::UnprovenSettled
    );
    let mut opaque = Progress::new(None);
    let unknown = Observation {
        status: TaskStatus::Unknown,
        through: None,
        transitions: vec![],
    };
    assert_eq!(opaque.observe(&unknown, 2), ObservationResult::Continue);
    assert_eq!(
        opaque.observe(&unknown, 2),
        ObservationResult::UnprovenSettled
    );
    let mut replaced = done.clone();
    replaced.through.as_mut().unwrap().stream = "new-epoch".into();
    assert_eq!(progress.observe(&replaced, 3), ObservationResult::Continue);
}

fn network_policy() -> Policy {
    Policy {
        interval: Duration::from_secs(10),
        hint_interval: Duration::from_millis(2),
        baseline: Duration::from_millis(50),
        read: Duration::from_secs(1),
        failures: 2,
        settling: 2,
    }
}

#[tokio::test]
async fn network_bursts_use_nonrotating_reads_without_admission_or_budget_consumption() {
    let fixture = Fixture::with_policy(network_policy());
    fixture.monitors.network_changed();
    assert_eq!(fixture.source.calls.load(Ordering::SeqCst), 0);
    let ticket = fixture.prepare(1).await;
    fixture.monitors.network_changed();
    assert_eq!(fixture.source.hint_calls.load(Ordering::SeqCst), 0);
    ticket.finish(&OperationOutcome::Accepted);
    fixture.source.calls(2).await;
    fixture.driver.agents(1).await;
    let (release, gate) = oneshot::channel();
    locked(&fixture.source.hint_steps).push_back(Step::Gate(gate));
    fixture.monitors.network_changed();
    fixture.source.hint_calls(1).await;
    for _ in 0..1000 {
        fixture.monitors.network_changed();
    }
    release
        .send(Err(ReadFailure::Unavailable))
        .unwrap_or_else(|_| panic!("hint was cancelled by another hint"));
    fixture.source.hint_calls(2).await;
    // The second failed hint would exhaust the regular failure budget.
    assert_eq!(locked(&fixture.monitors.inner.registry).monitors.len(), 1);
    assert_eq!(fixture.source.calls.load(Ordering::SeqCst), 2);
    *locked(&fixture.source.hint_fallback) = Ok(sample(TaskStatus::Idle, 0, &[]));
    for count in 3..=6 {
        fixture.monitors.network_changed();
        fixture.source.hint_calls(count).await;
    }
    assert_eq!(locked(&fixture.monitors.inner.registry).monitors.len(), 1);
    fixture.driver.agents(1).await;
    // A genuinely fresh complete turn remains authoritative, unlike stale Idle.
    *locked(&fixture.source.hint_fallback) = Ok(sample(
        TaskStatus::Completed,
        2,
        &[(1, TaskStatus::Running), (2, TaskStatus::Completed)],
    ));
    fixture.monitors.network_changed();
    fixture.stopped().await;
}

#[tokio::test]
async fn blocked_hint_yields_to_normal_deadline_before_its_read_budget_or_gate_release() {
    let fixture = Fixture::with_policy(Policy {
        interval: Duration::from_millis(30),
        failures: 1,
        ..network_policy()
    });
    fixture.prepare(1).await.finish(&OperationOutcome::Accepted);
    fixture.source.calls(2).await;
    let (release, gate) = oneshot::channel();
    locked(&fixture.source.hint_steps).push_back(Step::Gate(gate));
    fixture.monitors.network_changed();
    fixture.source.hint_calls(1).await;
    // policy.read is one second. The 30ms normal poll must cancel the blocked
    // hint, not wait for its gate or charge its timeout to failures=1.
    tokio::time::timeout(Duration::from_millis(500), fixture.source.calls(3))
        .await
        .expect("blocked hint postponed the scheduled normal poll");
    assert!(release.is_closed());
    assert_eq!(locked(&fixture.monitors.inner.registry).monitors.len(), 1);
    assert!(release
        .send(Ok(sample(TaskStatus::Completed, 2, &[])))
        .is_err());
    fixture.driver.agents(1).await;
}

#[tokio::test]
async fn network_hints_do_not_postpone_periodic_reads_and_host_switch_cancels_hint_work() {
    let fixture = Fixture::with_policy(Policy {
        interval: Duration::from_millis(30),
        ..network_policy()
    });
    fixture.prepare(1).await.finish(&OperationOutcome::Accepted);
    fixture.source.calls(2).await;
    let stop = CancellationToken::new();
    let worker_stop = stop.clone();
    let monitors = fixture.monitors.clone();
    let pump = tokio::spawn(async move {
        let mut ticks = tokio::time::interval(Duration::from_millis(1));
        loop {
            tokio::select! {
                _ = worker_stop.cancelled() => break,
                _ = ticks.tick() => monitors.network_changed(),
            }
        }
    });
    fixture.source.calls(3).await;
    stop.cancel();
    pump.await.unwrap();
    assert!(fixture.source.hint_calls.load(Ordering::SeqCst) > 0);
    let count = fixture.source.hint_calls.load(Ordering::SeqCst) + 1;
    let (mut release, gate) = oneshot::channel();
    locked(&fixture.source.hint_steps).push_back(Step::Gate(gate));
    fixture.monitors.network_changed();
    fixture.source.hint_calls(count).await;
    let mut other = session().workspace.host;
    other.host_id = "host-b".into();
    fixture.monitors.select_host(&other);
    fixture.driver.agents(0).await;
    tokio::time::timeout(Duration::from_secs(2), release.closed())
        .await
        .unwrap();
    assert!(release.send(Err(ReadFailure::Unavailable)).is_err());
    fixture.monitors.network_changed();
    assert!(locked(&fixture.monitors.inner.registry).monitors.is_empty());
    assert!(locked(&fixture.source.targets)
        .iter()
        .all(|target| target == &session()));
}

#[tokio::test]
async fn background_lease_precedes_dispatch_and_rejected_or_dropped_ticket_releases_it() {
    let fixture = Fixture::new();
    let ticket = fixture.prepare(1).await;
    fixture.driver.agents(1).await;
    assert_eq!(
        fixture.source.calls.load(Ordering::SeqCst),
        1,
        "only the bounded read baseline ran before dispatch"
    );
    ticket.finish(&OperationOutcome::Rejected {
        code: "known_preflight".into(),
        message: "not admitted".into(),
    });
    fixture.stopped().await;
    let ticket = fixture.prepare(2).await;
    fixture.driver.agents(1).await;
    drop(ticket);
    fixture.stopped().await;
    assert_eq!(fixture.source.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn non_message_operations_do_not_acquire_leases_or_read_a_host() {
    let fixture = Fixture::new();
    let operation = OperationRequest {
        operation_id: "existing-operation-id-0001".into(),
        mutation: Mutation::Cancel { session: session() },
    };
    assert!(fixture
        .monitors
        .prepare(&operation)
        .await
        .unwrap()
        .is_none());
    assert_eq!(fixture.source.calls.load(Ordering::SeqCst), 0);
    fixture.driver.agents(0).await;
}

#[tokio::test]
async fn concurrent_preparations_share_one_lease_and_unknown_only_starts_read_observation() {
    let fixture = Fixture::new();
    let (one, two) = (request(session(), 1), request(session(), 2));
    let (a, b) = tokio::join!(
        fixture.monitors.prepare(&one),
        fixture.monitors.prepare(&two)
    );
    let (a, b) = (a.unwrap().unwrap(), b.unwrap().unwrap());
    fixture.driver.agents(1).await;
    a.finish(&OperationOutcome::Accepted);
    b.finish(&OperationOutcome::Unknown {
        reason: "response lost".into(),
    });
    fixture.source.calls(3).await;
    assert_eq!(locked(&fixture.monitors.inner.registry).monitors.len(), 1);
    assert!(locked(&fixture.source.targets)
        .iter()
        .all(|target| *target == session()));
    let monitor = locked(&fixture.monitors.inner.registry)
        .monitors
        .get(&session())
        .unwrap()
        .clone();
    assert_eq!(
        locked(&monitor.state).operations,
        HashSet::from([one.operation_id, two.operation_id])
    );
    fixture.monitors.shutdown();
    fixture.stopped().await;
}

#[tokio::test]
async fn workspace_backend_session_identity_is_distinct_and_capacity_is_bounded() {
    let fixture = Fixture::new();
    let base = session();
    let mut targets = vec![base.clone()];
    let mut other = base.clone();
    other.workspace.workspace_id = "second-workspace".into();
    targets.push(other);
    let mut other = base.clone();
    other.session_id = "third-session".into();
    targets.push(other);
    let mut other = base.clone();
    other.workspace.backend_id = "third-backend".into();
    targets.push(other);
    let mut other = base.clone();
    other.workspace.backend_id = BackendId("another-backend".into());
    targets.push(other);
    let mut other = base.clone();
    other.workspace.workspace_id = "another-workspace".into();
    targets.push(other);
    let mut other = base.clone();
    other.session_id = "another-session".into();
    targets.push(other);
    let mut other = base.clone();
    other.session_id = "eighth-session".into();
    targets.push(other);
    let mut tickets = Vec::new();
    for (serial, target) in targets.into_iter().enumerate() {
        tickets.push(
            fixture
                .monitors
                .prepare(&request(target, serial))
                .await
                .unwrap()
                .unwrap(),
        );
    }
    fixture.driver.agents(8).await;
    let mut ninth = base;
    ninth.session_id = "ninth-session".into();
    assert!(fixture.monitors.prepare(&request(ninth, 9)).await.is_err());
    assert_eq!(
        locked(&fixture.monitors.inner.registry).monitors.len(),
        MAX_MONITORS
    );
    drop(tickets);
    fixture.stopped().await;
}

#[tokio::test]
async fn late_preflight_for_an_old_host_cannot_read_or_admit_after_an_explicit_selection() {
    let original = session();
    let mut variants = Vec::new();
    let mut host = original.workspace.host.clone();
    host.host_id = "selected-host-b".into();
    variants.push(host);
    let mut host = original.workspace.host.clone();
    host.connection.connection_id = "selected-account-b".into();
    variants.push(host);
    let mut host = original.workspace.host.clone();
    host.connection.provider_id = "selected-provider-b".into();
    variants.push(host);
    for host in variants {
        let fixture = Fixture::new();
        fixture.monitors.select_host(&host);
        assert!(fixture
            .monitors
            .prepare(&request(original.clone(), 1))
            .await
            .is_err());
        assert_eq!(
            fixture.source.calls.load(Ordering::SeqCst),
            0,
            "old prepare must fail before any Host read"
        );
        fixture.driver.agents(0).await;
        let mut selected = original.clone();
        selected.workspace.host = host;
        let ticket = fixture
            .monitors
            .prepare(&request(selected.clone(), 2))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(locked(&fixture.source.targets).as_slice(), &[selected]);
        ticket.finish(&OperationOutcome::Rejected {
            code: "test_preflight".into(),
            message: "not dispatched".into(),
        });
        fixture.stopped().await;
    }
}

#[tokio::test]
async fn host_selection_epoch_prevents_a_pending_old_ticket_from_reviving_after_a_b_a() {
    let fixture = Fixture::new();
    let old = fixture.prepare(1).await;
    let mut b = session().workspace.host;
    b.host_id = "host-b".into();
    fixture.monitors.select_host(&b);
    fixture.driver.agents(0).await;
    fixture.monitors.select_host(&session().workspace.host);
    old.finish(&OperationOutcome::Accepted);
    assert_eq!(fixture.source.calls.load(Ordering::SeqCst), 1);
    assert!(locked(&fixture.monitors.inner.registry).monitors.is_empty());
    let fresh = fixture.prepare(2).await;
    fresh.finish(&OperationOutcome::Rejected {
        code: "test_preflight".into(),
        message: "not dispatched".into(),
    });
    fixture.stopped().await;
}

#[tokio::test]
async fn selecting_b_interrupts_a_preparing_baseline_and_only_b_can_be_admitted() {
    let fixture = Fixture::new();
    fixture.source.blocked.store(true, Ordering::SeqCst);
    let monitors = fixture.monitors.clone();
    let preparing = tokio::spawn(async move { monitors.prepare(&request(session(), 1)).await });
    fixture.source.calls(1).await;
    let mut b = session();
    b.workspace.host.host_id = "host-b".into();
    fixture.monitors.select_host(&b.workspace.host);
    assert!(preparing.await.unwrap().is_err());
    fixture.stopped().await;
    fixture.source.blocked.store(false, Ordering::SeqCst);
    let current = fixture
        .monitors
        .prepare(&request(b, 2))
        .await
        .unwrap()
        .unwrap();
    current.finish(&OperationOutcome::Rejected {
        code: "test_preflight".into(),
        message: "not dispatched".into(),
    });
    fixture.stopped().await;
}

#[tokio::test]
async fn post_ack_stale_idle_holds_then_fresh_running_and_terminal_release_without_js() {
    let fixture = Fixture::new();
    fixture.source.push([Ok(sample(TaskStatus::Idle, 0, &[]))]);
    let ticket = fixture.prepare(1).await;
    fixture.source.push([
        Ok(sample(TaskStatus::Idle, 0, &[])),
        Ok(sample(TaskStatus::Running, 1, &[(1, TaskStatus::Running)])),
        Ok(sample(
            TaskStatus::Idle,
            2,
            &[(1, TaskStatus::Running), (2, TaskStatus::Idle)],
        )),
    ]);
    ticket.finish(&OperationOutcome::Accepted);
    fixture.stopped().await;
    assert_eq!(fixture.source.calls.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn unproven_idle_stops_only_local_monitor_after_bounded_settling() {
    let fixture = Fixture::new();
    *locked(&fixture.source.fallback) = Ok(sample(TaskStatus::Idle, 0, &[]));
    let ticket = fixture.prepare(1).await;
    ticket.finish(&OperationOutcome::Unknown {
        reason: "ambiguous request".into(),
    });
    fixture.stopped().await;
    assert_eq!(
        fixture.source.calls.load(Ordering::SeqCst),
        5,
        "one baseline and four settling observations"
    );
    assert!(locked(&fixture.source.targets)
        .iter()
        .all(|target| *target == session()));
}

#[tokio::test]
async fn finite_read_failures_and_timeouts_release_native_work_without_mutation_replay() {
    for blocked in [false, true] {
        let fixture = Fixture::new();
        let ticket = fixture.prepare(1).await;
        *locked(&fixture.source.fallback) = Err(ReadFailure::Unavailable);
        fixture.source.blocked.store(blocked, Ordering::SeqCst);
        ticket.finish(&OperationOutcome::Unknown {
            reason: "not a retry grant".into(),
        });
        fixture.stopped().await;
        assert_eq!(fixture.source.calls.load(Ordering::SeqCst), 4);
        assert_eq!(fixture.source.dropped.load(Ordering::SeqCst), 4);
    }
}

#[tokio::test]
async fn connection_change_stops_even_while_another_command_ticket_is_held() {
    let fixture = Fixture::new();
    let first = fixture.prepare(1).await;
    let pending = fixture.prepare(2).await;
    *locked(&fixture.source.fallback) = Err(ReadFailure::Namespace);
    first.finish(&OperationOutcome::Accepted);
    fixture.stopped().await;
    let count = fixture.source.calls.load(Ordering::SeqCst);
    pending.finish(&OperationOutcome::Unknown {
        reason: "old account".into(),
    });
    assert_eq!(fixture.source.calls.load(Ordering::SeqCst), count);
    assert!(locked(&fixture.monitors.inner.registry).monitors.is_empty());
}

#[tokio::test]
async fn ios_suspension_cancels_preparing_work_and_resume_never_restarts_it() {
    let fixture = Fixture::new();
    fixture.source.blocked.store(true, Ordering::SeqCst);
    let monitors = fixture.monitors.clone();
    let preparing = tokio::spawn(async move { monitors.prepare(&request(session(), 1)).await });
    fixture.source.calls(1).await;
    fixture.monitors.suspend();
    fixture.common.suspend();
    assert!(preparing.await.unwrap().is_err());
    fixture.stopped().await;
    assert!(fixture
        .monitors
        .prepare(&request(session(), 2))
        .await
        .is_err());
    let before = fixture.source.calls.load(Ordering::SeqCst);
    fixture.monitors.resume();
    assert_eq!(fixture.source.calls.load(Ordering::SeqCst), before);
    fixture.source.blocked.store(false, Ordering::SeqCst);
    let explicit = fixture.prepare(3).await;
    explicit.finish(&OperationOutcome::Rejected {
        code: "known_refusal".into(),
        message: "not dispatched".into(),
    });
    fixture.stopped().await;
}

#[tokio::test]
async fn common_ios_token_and_logout_shutdown_stop_read_futures_and_release_held_leases() {
    for shutdown in [false, true] {
        let fixture = Fixture::new();
        let ticket = fixture.prepare(1).await;
        fixture.source.blocked.store(true, Ordering::SeqCst);
        ticket.finish(&OperationOutcome::Accepted);
        fixture.source.calls(2).await;
        if shutdown {
            fixture.monitors.shutdown();
        } else {
            fixture.common.suspend();
        }
        fixture.stopped().await;
        assert_eq!(fixture.source.dropped.load(Ordering::SeqCst), 2);
    }
    let fixture = Fixture::new();
    let pending = fixture.prepare(1).await;
    fixture
        .monitors
        .stop_connection(&session().workspace.host.connection);
    fixture.driver.agents(0).await;
    pending.finish(&OperationOutcome::Accepted);
    assert!(locked(&fixture.monitors.inner.registry).monitors.is_empty());
    fixture.monitors.shutdown();
    assert!(fixture
        .monitors
        .prepare(&request(session(), 2))
        .await
        .is_err());
}

#[tokio::test]
async fn an_old_terminal_read_cannot_retire_a_newer_send_on_the_same_session() {
    let fixture = Fixture::new();
    fixture.source.push([Ok(sample(TaskStatus::Idle, 0, &[]))]);
    let first = fixture.prepare(1).await;
    let (release, waiting) = oneshot::channel();
    fixture.source.push([Ok(sample(
        TaskStatus::Running,
        1,
        &[(1, TaskStatus::Running)],
    ))]);
    locked(&fixture.source.steps).push_back(Step::Gate(waiting));
    *locked(&fixture.source.fallback) =
        Ok(sample(TaskStatus::Running, 1, &[(1, TaskStatus::Running)]));
    first.finish(&OperationOutcome::Accepted);
    fixture.source.calls(3).await;
    let next = fixture.prepare(2).await;
    next.finish(&OperationOutcome::Accepted);
    release
        .send(Ok(sample(
            TaskStatus::Idle,
            2,
            &[(1, TaskStatus::Running), (2, TaskStatus::Idle)],
        )))
        .unwrap_or_else(|_| panic!("old read cancelled unexpectedly"));
    fixture.source.calls(5).await;
    fixture.driver.agents(1).await;
    assert_eq!(locked(&fixture.monitors.inner.registry).monitors.len(), 1);
    fixture.source.push([
        Ok(sample(TaskStatus::Running, 3, &[(3, TaskStatus::Running)])),
        Ok(sample(
            TaskStatus::Idle,
            4,
            &[(3, TaskStatus::Running), (4, TaskStatus::Idle)],
        )),
    ]);
    fixture.stopped().await;
}

#[tokio::test]
async fn dropping_an_unpolled_observer_releases_native_work_even_with_a_held_ticket() {
    let fixture = Fixture::new();
    let ticket = fixture.prepare(1).await;
    let owner = ObserverOwner {
        manager: Arc::downgrade(&fixture.monitors.inner),
        monitor: ticket.monitor.clone(),
    };
    let never_polled = observe(owner, fixture.source.clone(), fixture.monitors.inner.policy);
    drop(never_polled);
    fixture.driver.agents(0).await;
    assert!(locked(&fixture.monitors.inner.registry).monitors.is_empty());
    assert_eq!(fixture.source.calls.load(Ordering::SeqCst), 1);
    drop(ticket);
}

#[test]
fn observation_rejects_wrong_session_and_does_not_retain_history_bodies() {
    let expected = session();
    let mut other = expected.clone();
    other.session_id = "other-session".into();
    let snapshot = SessionSnapshot {
        info: SessionInfo {
            session: other,
            title: "not retained".into(),
            status: TaskStatus::Idle,
        },
        events: vec![],
        cursor: None,
        context_usage: None,
    };
    assert!(matches!(
        Observation::snapshot(&expected, snapshot),
        Err(ReadFailure::Namespace)
    ));
    assert!(Point::parse(
        &expected,
        &EventCursor {
            stream_id: "opaque".into(),
            value: "+1".into()
        }
    )
    .is_none());
    let mut other_provider = expected.clone();
    other_provider.workspace.host.connection.provider_id = "opaque-provider".into();
    assert!(Point::parse(
        &other_provider,
        &EventCursor {
            stream_id: "opaque".into(),
            value: "1".into()
        }
    )
    .is_none());
}
