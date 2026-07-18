use std::{
    collections::HashSet,
    fmt,
    panic::AssertUnwindSafe,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use chrono::{
    DateTime, Datelike, Duration as ChronoDuration, LocalResult, NaiveTime, TimeZone, Utc, Weekday,
};
use chrono_tz::Tz;
use futures_util::FutureExt;
use phi::{CapabilityMode, Content, Workspace};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::{
    sync::{Mutex, Notify, OwnedSemaphorePermit, Semaphore, watch},
    task::{JoinHandle, JoinSet},
};
use uuid::Uuid;

use crate::{
    runtime::{RunId, RuntimeEventKind, SessionId},
    service::{ApplicationService, ServiceError},
    store::{ScheduledTaskStore, ScheduledTaskStoreError},
};

pub(crate) const MAX_SCHEDULED_TASKS: usize = 1_000;
const MAX_CONCURRENT_RUNS: usize = 8;
const MAX_NAME_CHARS: usize = 100;
const MAX_PROMPT_CHARS: usize = 20_000;
const MAX_INTERVAL_SECONDS: u64 = 10 * 366 * 24 * 60 * 60;
const SCHEDULER_TICK: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ScheduledTaskId(Uuid);

impl ScheduledTaskId {
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    pub fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl Default for ScheduledTaskId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for ScheduledTaskId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl std::str::FromStr for ScheduledTaskId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.parse().map(Self)
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduledWeekday {
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
    Sunday,
}

impl ScheduledWeekday {
    fn matches(self, weekday: Weekday) -> bool {
        matches!(
            (self, weekday),
            (Self::Monday, Weekday::Mon)
                | (Self::Tuesday, Weekday::Tue)
                | (Self::Wednesday, Weekday::Wed)
                | (Self::Thursday, Weekday::Thu)
                | (Self::Friday, Weekday::Fri)
                | (Self::Saturday, Weekday::Sat)
                | (Self::Sunday, Weekday::Sun)
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduledIntervalUnit {
    Minutes,
    Hours,
    Days,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ScheduledTaskSchedule {
    Daily {
        /// Local wall-clock time in stable `HH:MM` form.
        time: String,
        weekdays: Vec<ScheduledWeekday>,
        /// IANA time-zone name supplied by the client (for example
        /// `Asia/Singapore`).
        timezone: String,
    },
    Interval {
        every: u32,
        unit: ScheduledIntervalUnit,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduledRunOutcome {
    Running,
    Succeeded,
    Failed,
    Stopped,
    Interrupted,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ScheduledTaskRun {
    pub scheduled_for: DateTime<Utc>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub outcome: ScheduledRunOutcome,
    pub session_id: Option<SessionId>,
    pub error: Option<String>,
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ScheduledTask {
    pub id: ScheduledTaskId,
    pub name: String,
    pub prompt: String,
    pub workspace: Workspace,
    pub profile_id: String,
    pub agent_profile_id: String,
    pub capability_mode: Option<CapabilityMode>,
    pub schedule: ScheduledTaskSchedule,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub next_run_at: Option<DateTime<Utc>>,
    pub last_run: Option<ScheduledTaskRun>,
    #[serde(default)]
    pub skipped_runs: u64,
    pub revision: u64,
}

impl fmt::Debug for ScheduledTask {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ScheduledTask")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("prompt", &"[REDACTED]")
            .field("workspace", &self.workspace)
            .field("profile_id", &self.profile_id)
            .field("agent_profile_id", &self.agent_profile_id)
            .field("capability_mode", &self.capability_mode)
            .field("schedule", &self.schedule)
            .field("enabled", &self.enabled)
            .field("created_at", &self.created_at)
            .field("updated_at", &self.updated_at)
            .field("next_run_at", &self.next_run_at)
            .field("last_run", &self.last_run)
            .field("skipped_runs", &self.skipped_runs)
            .field("revision", &self.revision)
            .finish()
    }
}

#[derive(Clone, Debug)]
pub struct CreateScheduledTask {
    pub name: String,
    pub prompt: String,
    pub workspace: Workspace,
    pub profile_id: String,
    pub agent_profile_id: String,
    pub capability_mode: Option<CapabilityMode>,
    pub schedule: ScheduledTaskSchedule,
}

#[derive(Clone, Copy, Debug)]
pub struct UpdateScheduledTask {
    pub enabled: bool,
    pub expected_revision: Option<u64>,
}

#[derive(Default)]
struct CoordinatorState {
    running: HashSet<ScheduledTaskId>,
}

/// Owns scheduled-task admission and execution for one daemon process.
///
/// A schedule occurrence is durably advanced before its session is created.
/// This prevents process restart from automatically replaying a prompt whose
/// external side effects may already have started. A crash between admission
/// and completion is surfaced as an `interrupted` last run on the next start.
pub struct ScheduledTaskManager {
    service: Arc<ApplicationService>,
    store: Arc<dyn ScheduledTaskStore>,
    coordinator: Mutex<CoordinatorState>,
    executions: Mutex<JoinSet<()>>,
    execution_slots: Arc<Semaphore>,
    wake: Notify,
    shutdown_tx: watch::Sender<bool>,
    worker: Mutex<Option<JoinHandle<()>>>,
    started: AtomicBool,
    closing: AtomicBool,
}

impl ScheduledTaskManager {
    pub fn new(service: Arc<ApplicationService>, store: Arc<dyn ScheduledTaskStore>) -> Self {
        let (shutdown_tx, _) = watch::channel(false);
        Self {
            service,
            store,
            coordinator: Mutex::new(CoordinatorState::default()),
            executions: Mutex::new(JoinSet::new()),
            execution_slots: Arc::new(Semaphore::new(MAX_CONCURRENT_RUNS)),
            wake: Notify::new(),
            shutdown_tx,
            worker: Mutex::new(None),
            started: AtomicBool::new(false),
            closing: AtomicBool::new(false),
        }
    }

    pub async fn start(self: &Arc<Self>) -> Result<(), ScheduledTaskError> {
        if self
            .started
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Ok(());
        }
        if let Err(error) = self.recover_interrupted_runs().await {
            self.started.store(false, Ordering::Release);
            return Err(error);
        }

        let manager = Arc::clone(self);
        let mut shutdown = self.shutdown_tx.subscribe();
        let worker = tokio::spawn(async move {
            loop {
                if *shutdown.borrow() {
                    break;
                }
                if let Err(error) = manager.dispatch_due_at(Utc::now()).await {
                    tracing::error!(error = %error, "scheduled-task dispatch failed");
                }
                manager.reap_executions().await;
                tokio::select! {
                    _ = tokio::time::sleep(SCHEDULER_TICK) => {}
                    _ = manager.wake.notified() => {}
                    changed = shutdown.changed() => {
                        if changed.is_err() || *shutdown.borrow() {
                            break;
                        }
                    }
                }
            }
        });
        *self.worker.lock().await = Some(worker);
        Ok(())
    }

    pub async fn shutdown(&self) {
        if self.closing.swap(true, Ordering::AcqRel) {
            return;
        }
        let _ = self.shutdown_tx.send(true);
        self.wake.notify_waiters();
        if let Some(worker) = self.worker.lock().await.take() {
            let _ = worker.await;
        }

        {
            let mut executions = self.executions.lock().await;
            executions.abort_all();
            while let Some(result) = executions.join_next().await {
                if let Err(error) = result
                    && !error.is_cancelled()
                {
                    tracing::warn!(error = %error, "scheduled-task execution task failed");
                }
            }
        }
        if let Err(error) = self
            .interrupt_running_tasks("daemon shut down before the run completed")
            .await
        {
            tracing::warn!(error = %error, "could not mark scheduled tasks interrupted");
        }
    }

    pub async fn create_task(
        &self,
        request: CreateScheduledTask,
    ) -> Result<ScheduledTask, ScheduledTaskError> {
        self.ensure_open()?;
        let request = normalize_create(request)?;
        if self
            .service
            .provider_config_for(&request.profile_id)
            .await?
            .is_none()
        {
            return Err(ScheduledTaskError::ProviderNotFound {
                profile_id: request.profile_id,
            });
        }
        if self
            .service
            .agent_profile(&request.agent_profile_id)
            .await?
            .is_none()
        {
            return Err(ScheduledTaskError::AgentProfileNotFound {
                agent_profile_id: request.agent_profile_id,
            });
        }

        let _coordinator = self.coordinator.lock().await;
        if self.store.list_tasks().await?.len() >= MAX_SCHEDULED_TASKS {
            return Err(ScheduledTaskError::TaskLimit {
                capacity: MAX_SCHEDULED_TASKS,
            });
        }
        let now = Utc::now();
        let task = ScheduledTask {
            id: ScheduledTaskId::new(),
            name: request.name,
            prompt: request.prompt,
            workspace: request.workspace,
            profile_id: request.profile_id,
            agent_profile_id: request.agent_profile_id,
            capability_mode: request.capability_mode,
            next_run_at: Some(initial_next_run(&request.schedule, now)?),
            schedule: request.schedule,
            enabled: true,
            created_at: now,
            updated_at: now,
            last_run: None,
            skipped_runs: 0,
            revision: 1,
        };
        self.store.create_task(task.clone()).await?;
        drop(_coordinator);
        self.wake.notify_one();
        Ok(task)
    }

    pub async fn list_tasks(&self) -> Result<Vec<ScheduledTask>, ScheduledTaskError> {
        let mut tasks = self.store.list_tasks().await?;
        tasks.sort_unstable_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| right.id.cmp(&left.id))
        });
        Ok(tasks)
    }

    pub async fn get_task(
        &self,
        task_id: ScheduledTaskId,
    ) -> Result<ScheduledTask, ScheduledTaskError> {
        self.store
            .get_task(task_id)
            .await?
            .ok_or(ScheduledTaskError::NotFound { task_id })
    }

    pub async fn update_task(
        &self,
        task_id: ScheduledTaskId,
        update: UpdateScheduledTask,
    ) -> Result<ScheduledTask, ScheduledTaskError> {
        self.ensure_open()?;
        let _coordinator = self.coordinator.lock().await;
        let mut task = self
            .store
            .get_task(task_id)
            .await?
            .ok_or(ScheduledTaskError::NotFound { task_id })?;
        if let Some(expected) = update.expected_revision
            && task.revision != expected
        {
            return Err(ScheduledTaskError::RevisionConflict {
                task_id,
                expected,
                actual: task.revision,
            });
        }
        if task.enabled != update.enabled {
            let now = Utc::now();
            task.enabled = update.enabled;
            task.next_run_at = if update.enabled {
                Some(initial_next_run(&task.schedule, now)?)
            } else {
                None
            };
            task.updated_at = now;
            task.revision = task
                .revision
                .checked_add(1)
                .ok_or(ScheduledTaskError::RevisionExhausted { task_id })?;
            self.store.update_task(task.clone()).await?;
        }
        drop(_coordinator);
        self.wake.notify_one();
        Ok(task)
    }

    pub async fn delete_task(&self, task_id: ScheduledTaskId) -> Result<(), ScheduledTaskError> {
        self.ensure_open()?;
        let _coordinator = self.coordinator.lock().await;
        if !self.store.delete_task(task_id).await? {
            return Err(ScheduledTaskError::NotFound { task_id });
        }
        drop(_coordinator);
        self.wake.notify_one();
        Ok(())
    }

    pub async fn run_now(
        self: &Arc<Self>,
        task_id: ScheduledTaskId,
    ) -> Result<(), ScheduledTaskError> {
        self.ensure_open()?;
        let permit = Arc::clone(&self.execution_slots)
            .try_acquire_owned()
            .map_err(|_| ScheduledTaskError::RunCapacity {
                capacity: MAX_CONCURRENT_RUNS,
            })?;
        let now = Utc::now();
        let task = {
            let mut coordinator = self.coordinator.lock().await;
            if coordinator.running.contains(&task_id) {
                return Err(ScheduledTaskError::AlreadyRunning { task_id });
            }
            let mut task = self
                .store
                .get_task(task_id)
                .await?
                .ok_or(ScheduledTaskError::NotFound { task_id })?;
            task.last_run = Some(running_record(now, now));
            self.store.update_task(task.clone()).await?;
            coordinator.running.insert(task_id);
            task
        };
        self.spawn_execution(task, now, permit).await;
        Ok(())
    }

    async fn dispatch_due_at(
        self: &Arc<Self>,
        now: DateTime<Utc>,
    ) -> Result<(), ScheduledTaskError> {
        let mut starts = Vec::new();
        {
            let mut coordinator = self.coordinator.lock().await;
            let mut tasks = self.store.list_tasks().await?;
            tasks.sort_unstable_by_key(|task| task.next_run_at);
            for mut task in tasks {
                let Some(scheduled_for) = task.next_run_at else {
                    continue;
                };
                if !task.enabled || scheduled_for > now {
                    continue;
                }
                if coordinator.running.contains(&task.id) {
                    task.next_run_at = Some(advance_past(&task.schedule, scheduled_for, now)?);
                    task.skipped_runs = task.skipped_runs.saturating_add(1);
                    self.store.update_task(task).await?;
                    continue;
                }
                let Ok(permit) = Arc::clone(&self.execution_slots).try_acquire_owned() else {
                    break;
                };
                task.next_run_at = Some(advance_past(&task.schedule, scheduled_for, now)?);
                task.last_run = Some(running_record(scheduled_for, now));
                self.store.update_task(task.clone()).await?;
                coordinator.running.insert(task.id);
                starts.push((task, scheduled_for, permit));
            }
        }
        for (task, scheduled_for, permit) in starts {
            self.spawn_execution(task, scheduled_for, permit).await;
        }
        Ok(())
    }

    async fn spawn_execution(
        self: &Arc<Self>,
        task: ScheduledTask,
        scheduled_for: DateTime<Utc>,
        permit: OwnedSemaphorePermit,
    ) {
        let manager = Arc::clone(self);
        let task_id = task.id;
        let mut executions = self.executions.lock().await;
        reap_join_set(&mut executions);
        executions.spawn(async move {
            let _permit = permit;
            let completion = AssertUnwindSafe(manager.execute_task(task, scheduled_for))
                .catch_unwind()
                .await
                .unwrap_or_else(|_| TaskCompletion::interrupted("scheduled-task runtime panicked"));
            manager
                .finish_execution(task_id, scheduled_for, completion)
                .await;
        });
    }

    async fn execute_task(
        &self,
        task: ScheduledTask,
        scheduled_for: DateTime<Utc>,
    ) -> TaskCompletion {
        let prepared = match self
            .service
            .prepare_session_configured_in_workspace(
                task.profile_id.clone(),
                task.agent_profile_id.clone(),
                task.capability_mode,
                task.workspace.clone(),
            )
            .await
        {
            Ok(prepared) => prepared,
            Err(error) => return TaskCompletion::failed(error.to_string()),
        };
        let handle = match self.service.activate_session(&prepared).await {
            Ok(handle) => handle,
            Err(error) => {
                self.service.discard_prepared(&prepared).await;
                return TaskCompletion::failed(error.to_string());
            }
        };
        let session_id = handle.session_id();
        if let Err(error) = handle.set_title(task.name).await {
            self.cleanup_failed_session(session_id).await;
            return TaskCompletion::failed(error.to_string());
        }
        let mut events = handle.subscribe();
        let queued = match handle.enqueue_prompt(Content::text(task.prompt)).await {
            Ok(queued) => queued,
            Err(error) => {
                self.cleanup_failed_session(session_id).await;
                return TaskCompletion::failed(error.to_string());
            }
        };
        if let Err(error) = self
            .record_session(task.id, scheduled_for, session_id)
            .await
        {
            tracing::warn!(
                task_id = %task.id,
                %session_id,
                error = %error,
                "could not persist the scheduled run session id"
            );
        }
        wait_for_run(&mut events, queued.run_id).await
    }

    async fn cleanup_failed_session(&self, session_id: SessionId) {
        if let Err(error) = self.service.delete_session(session_id).await {
            tracing::warn!(
                %session_id,
                error = %error,
                "could not clean up a scheduled session that failed before prompt admission"
            );
        }
    }

    async fn record_session(
        &self,
        task_id: ScheduledTaskId,
        scheduled_for: DateTime<Utc>,
        session_id: SessionId,
    ) -> Result<(), ScheduledTaskError> {
        let _coordinator = self.coordinator.lock().await;
        let Some(mut task) = self.store.get_task(task_id).await? else {
            return Ok(());
        };
        if let Some(run) = task.last_run.as_mut()
            && run.scheduled_for == scheduled_for
            && run.outcome == ScheduledRunOutcome::Running
        {
            run.session_id = Some(session_id);
            self.store.update_task(task).await?;
        }
        Ok(())
    }

    async fn finish_execution(
        &self,
        task_id: ScheduledTaskId,
        scheduled_for: DateTime<Utc>,
        completion: TaskCompletion,
    ) {
        let result = async {
            let mut coordinator = self.coordinator.lock().await;
            if let Some(mut task) = self.store.get_task(task_id).await?
                && let Some(run) = task.last_run.as_mut()
                && run.scheduled_for == scheduled_for
                && run.outcome == ScheduledRunOutcome::Running
            {
                run.finished_at = Some(Utc::now());
                run.outcome = completion.outcome;
                run.error = completion.error;
                self.store.update_task(task).await?;
            }
            coordinator.running.remove(&task_id);
            Ok::<(), ScheduledTaskError>(())
        }
        .await;
        if let Err(error) = result {
            tracing::error!(
                %task_id,
                error = %error,
                "could not persist scheduled-task completion"
            );
            self.coordinator.lock().await.running.remove(&task_id);
        }
        self.wake.notify_one();
    }

    async fn recover_interrupted_runs(&self) -> Result<(), ScheduledTaskError> {
        let _coordinator = self.coordinator.lock().await;
        let now = Utc::now();
        for mut task in self.store.list_tasks().await? {
            if let Some(run) = task.last_run.as_mut()
                && run.outcome == ScheduledRunOutcome::Running
            {
                run.finished_at = Some(now);
                run.outcome = ScheduledRunOutcome::Interrupted;
                run.error = Some("daemon restarted before the run completed".to_owned());
                self.store.update_task(task).await?;
            }
        }
        Ok(())
    }

    async fn interrupt_running_tasks(&self, message: &str) -> Result<(), ScheduledTaskError> {
        let mut coordinator = self.coordinator.lock().await;
        let running = std::mem::take(&mut coordinator.running);
        let now = Utc::now();
        for task_id in running {
            let Some(mut task) = self.store.get_task(task_id).await? else {
                continue;
            };
            if let Some(run) = task.last_run.as_mut()
                && run.outcome == ScheduledRunOutcome::Running
            {
                run.finished_at = Some(now);
                run.outcome = ScheduledRunOutcome::Interrupted;
                run.error = Some(message.to_owned());
                self.store.update_task(task).await?;
            }
        }
        Ok(())
    }

    async fn reap_executions(&self) {
        let mut executions = self.executions.lock().await;
        reap_join_set(&mut executions);
    }

    fn ensure_open(&self) -> Result<(), ScheduledTaskError> {
        if self.closing.load(Ordering::Acquire) {
            Err(ScheduledTaskError::ShuttingDown)
        } else {
            Ok(())
        }
    }
}

fn reap_join_set(executions: &mut JoinSet<()>) {
    while let Some(result) = executions.try_join_next() {
        if let Err(error) = result
            && !error.is_cancelled()
        {
            tracing::warn!(error = %error, "scheduled-task execution task failed");
        }
    }
}

async fn wait_for_run(
    events: &mut tokio::sync::broadcast::Receiver<crate::runtime::RuntimeEvent>,
    run_id: RunId,
) -> TaskCompletion {
    loop {
        match events.recv().await {
            Ok(event) if event.run_id == Some(run_id) => match event.kind {
                RuntimeEventKind::RunCompleted { .. } => return TaskCompletion::succeeded(),
                RuntimeEventKind::RunStopped { .. } => return TaskCompletion::stopped(),
                RuntimeEventKind::RunFailed { message, .. } => {
                    return TaskCompletion::failed(message);
                }
                _ => {}
            },
            Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                return TaskCompletion::interrupted(
                    "session actor closed before publishing a terminal run event",
                );
            }
        }
    }
}

struct TaskCompletion {
    outcome: ScheduledRunOutcome,
    error: Option<String>,
}

impl TaskCompletion {
    fn succeeded() -> Self {
        Self {
            outcome: ScheduledRunOutcome::Succeeded,
            error: None,
        }
    }

    fn stopped() -> Self {
        Self {
            outcome: ScheduledRunOutcome::Stopped,
            error: None,
        }
    }

    fn failed(error: impl Into<String>) -> Self {
        Self {
            outcome: ScheduledRunOutcome::Failed,
            error: Some(error.into()),
        }
    }

    fn interrupted(error: impl Into<String>) -> Self {
        Self {
            outcome: ScheduledRunOutcome::Interrupted,
            error: Some(error.into()),
        }
    }
}

fn running_record(scheduled_for: DateTime<Utc>, started_at: DateTime<Utc>) -> ScheduledTaskRun {
    ScheduledTaskRun {
        scheduled_for,
        started_at,
        finished_at: None,
        outcome: ScheduledRunOutcome::Running,
        session_id: None,
        error: None,
    }
}

fn normalize_create(
    request: CreateScheduledTask,
) -> Result<CreateScheduledTask, ScheduledTaskError> {
    let name = request.name.trim();
    if name.is_empty() {
        return Err(invalid("name", "must not be empty"));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(invalid(
            "name",
            format!("must not exceed {MAX_NAME_CHARS} characters"),
        ));
    }
    if name.chars().any(char::is_control) {
        return Err(invalid("name", "must not contain control characters"));
    }
    let prompt = request.prompt.trim();
    if prompt.is_empty() {
        return Err(invalid("prompt", "must not be empty"));
    }
    if prompt.chars().count() > MAX_PROMPT_CHARS {
        return Err(invalid(
            "prompt",
            format!("must not exceed {MAX_PROMPT_CHARS} characters"),
        ));
    }
    let profile_id = normalize_identifier("profile_id", &request.profile_id)?;
    let agent_profile_id = normalize_identifier("agent_profile_id", &request.agent_profile_id)?;
    Ok(CreateScheduledTask {
        name: name.to_owned(),
        prompt: prompt.to_owned(),
        workspace: request.workspace,
        profile_id,
        agent_profile_id,
        capability_mode: request.capability_mode,
        schedule: normalize_schedule(request.schedule)?,
    })
}

fn normalize_identifier(field: &'static str, value: &str) -> Result<String, ScheduledTaskError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(invalid(field, "must not be empty"));
    }
    if value.len() > 128 {
        return Err(invalid(field, "must not exceed 128 bytes"));
    }
    if value.chars().any(char::is_control) {
        return Err(invalid(field, "must not contain control characters"));
    }
    Ok(value.to_owned())
}

fn normalize_schedule(
    schedule: ScheduledTaskSchedule,
) -> Result<ScheduledTaskSchedule, ScheduledTaskError> {
    match schedule {
        ScheduledTaskSchedule::Daily {
            time,
            mut weekdays,
            timezone,
        } => {
            let local_time = parse_time(&time)?;
            if weekdays.is_empty() {
                return Err(invalid("schedule.weekdays", "must not be empty"));
            }
            weekdays.sort_unstable();
            weekdays.dedup();
            parse_timezone(&timezone)?;
            Ok(ScheduledTaskSchedule::Daily {
                time: local_time.format("%H:%M").to_string(),
                weekdays,
                timezone,
            })
        }
        ScheduledTaskSchedule::Interval { every, unit } => {
            interval_duration(every, unit)?;
            Ok(ScheduledTaskSchedule::Interval { every, unit })
        }
    }
}

fn initial_next_run(
    schedule: &ScheduledTaskSchedule,
    now: DateTime<Utc>,
) -> Result<DateTime<Utc>, ScheduledTaskError> {
    match schedule {
        ScheduledTaskSchedule::Daily { .. } => next_daily_after(schedule, now),
        ScheduledTaskSchedule::Interval { every, unit } => now
            .checked_add_signed(interval_duration(*every, *unit)?)
            .ok_or_else(|| invalid("schedule", "next run is outside the supported time range")),
    }
}

fn advance_past(
    schedule: &ScheduledTaskSchedule,
    scheduled_for: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<DateTime<Utc>, ScheduledTaskError> {
    match schedule {
        ScheduledTaskSchedule::Daily { .. } => next_daily_after(schedule, now),
        ScheduledTaskSchedule::Interval { every, unit } => {
            let duration = interval_duration(*every, *unit)?;
            let duration_ms = duration.num_milliseconds();
            let elapsed_ms = now
                .signed_duration_since(scheduled_for)
                .num_milliseconds()
                .max(0);
            let steps = elapsed_ms
                .checked_div(duration_ms)
                .and_then(|steps| steps.checked_add(1))
                .ok_or_else(|| invalid("schedule", "could not advance interval"))?;
            let offset_ms = duration_ms
                .checked_mul(steps)
                .ok_or_else(|| invalid("schedule", "next run is outside the supported range"))?;
            scheduled_for
                .checked_add_signed(ChronoDuration::milliseconds(offset_ms))
                .ok_or_else(|| invalid("schedule", "next run is outside the supported range"))
        }
    }
}

fn next_daily_after(
    schedule: &ScheduledTaskSchedule,
    after: DateTime<Utc>,
) -> Result<DateTime<Utc>, ScheduledTaskError> {
    let ScheduledTaskSchedule::Daily {
        time,
        weekdays,
        timezone,
    } = schedule
    else {
        return Err(invalid("schedule", "expected a daily schedule"));
    };
    let local_time = parse_time(time)?;
    let timezone = parse_timezone(timezone)?;
    let local_date = after.with_timezone(&timezone).date_naive();
    for offset in 0..=8 {
        let date = local_date
            .checked_add_signed(ChronoDuration::days(offset))
            .ok_or_else(|| invalid("schedule", "next run is outside the supported range"))?;
        if !weekdays
            .iter()
            .copied()
            .any(|weekday| weekday.matches(date.weekday()))
        {
            continue;
        }
        let local = date.and_time(local_time);
        let candidate = match timezone.from_local_datetime(&local) {
            LocalResult::Single(candidate) => candidate,
            // Run once on a fall-back day by choosing the earlier occurrence.
            LocalResult::Ambiguous(first, second) => first.min(second),
            // A wall-clock time in a spring-forward gap has no safe exact
            // interpretation, so skip that local day.
            LocalResult::None => continue,
        }
        .with_timezone(&Utc);
        if candidate > after {
            return Ok(candidate);
        }
    }
    Err(invalid(
        "schedule",
        "could not find the next selected local day",
    ))
}

fn parse_time(value: &str) -> Result<NaiveTime, ScheduledTaskError> {
    NaiveTime::parse_from_str(value, "%H:%M")
        .map_err(|_| invalid("schedule.time", "must use 24-hour HH:MM format"))
}

fn parse_timezone(value: &str) -> Result<Tz, ScheduledTaskError> {
    value
        .parse::<Tz>()
        .map_err(|_| invalid("schedule.timezone", "must be a valid IANA time-zone name"))
}

fn interval_duration(
    every: u32,
    unit: ScheduledIntervalUnit,
) -> Result<ChronoDuration, ScheduledTaskError> {
    if every == 0 {
        return Err(invalid("schedule.every", "must be greater than zero"));
    }
    let unit_seconds = match unit {
        ScheduledIntervalUnit::Minutes => 60_u64,
        ScheduledIntervalUnit::Hours => 60 * 60,
        ScheduledIntervalUnit::Days => 24 * 60 * 60,
    };
    let seconds = u64::from(every)
        .checked_mul(unit_seconds)
        .filter(|seconds| *seconds <= MAX_INTERVAL_SECONDS)
        .ok_or_else(|| invalid("schedule.every", "interval must not exceed ten years"))?;
    let seconds =
        i64::try_from(seconds).map_err(|_| invalid("schedule.every", "interval is too large"))?;
    Ok(ChronoDuration::seconds(seconds))
}

fn invalid(field: &'static str, message: impl Into<String>) -> ScheduledTaskError {
    ScheduledTaskError::InvalidField {
        field,
        message: message.into(),
    }
}

/// Validates data loaded from the scheduled-task store without normalizing or
/// silently changing its persisted representation.
pub(crate) fn validate_persisted_task(task: &ScheduledTask) -> Result<(), String> {
    let normalized = normalize_create(CreateScheduledTask {
        name: task.name.clone(),
        prompt: task.prompt.clone(),
        workspace: task.workspace.clone(),
        profile_id: task.profile_id.clone(),
        agent_profile_id: task.agent_profile_id.clone(),
        capability_mode: task.capability_mode,
        schedule: task.schedule.clone(),
    })
    .map_err(|error| error.to_string())?;
    if normalized.name != task.name
        || normalized.prompt != task.prompt
        || normalized.profile_id != task.profile_id
        || normalized.agent_profile_id != task.agent_profile_id
        || normalized.schedule != task.schedule
    {
        return Err("task contains non-normalized fields".to_owned());
    }
    if task.revision == 0 {
        return Err("task revision must be greater than zero".to_owned());
    }
    if task.updated_at < task.created_at {
        return Err("updated_at precedes created_at".to_owned());
    }
    if task.enabled != task.next_run_at.is_some() {
        return Err("enabled and next_run_at are inconsistent".to_owned());
    }
    if let Some(run) = &task.last_run {
        if run.started_at < run.scheduled_for {
            return Err("last run started before its scheduled time".to_owned());
        }
        if run.outcome == ScheduledRunOutcome::Running && run.finished_at.is_some() {
            return Err("running last run has a finish time".to_owned());
        }
        if run.outcome != ScheduledRunOutcome::Running && run.finished_at.is_none() {
            return Err("terminal last run has no finish time".to_owned());
        }
        if run
            .finished_at
            .is_some_and(|finished_at| finished_at < run.started_at)
        {
            return Err("last run finished before it started".to_owned());
        }
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum ScheduledTaskError {
    #[error("scheduled-task manager is shutting down")]
    ShuttingDown,

    #[error("scheduled task {task_id} was not found")]
    NotFound { task_id: ScheduledTaskId },

    #[error("scheduled task {task_id} is already running")]
    AlreadyRunning { task_id: ScheduledTaskId },

    #[error("scheduled-task run capacity is full (capacity {capacity})")]
    RunCapacity { capacity: usize },

    #[error("scheduled-task limit reached (capacity {capacity})")]
    TaskLimit { capacity: usize },

    #[error(
        "scheduled task {task_id} revision conflict: expected {expected}, current revision is {actual}"
    )]
    RevisionConflict {
        task_id: ScheduledTaskId,
        expected: u64,
        actual: u64,
    },

    #[error("scheduled task {task_id} revision is exhausted")]
    RevisionExhausted { task_id: ScheduledTaskId },

    #[error("invalid scheduled-task field {field}: {message}")]
    InvalidField {
        field: &'static str,
        message: String,
    },

    #[error("Provider profile {profile_id:?} was not found")]
    ProviderNotFound { profile_id: String },

    #[error("Agent Profile {agent_profile_id:?} was not found")]
    AgentProfileNotFound { agent_profile_id: String },

    #[error(transparent)]
    Store(#[from] ScheduledTaskStoreError),

    #[error(transparent)]
    Service(#[from] ServiceError),
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn weekdays() -> Vec<ScheduledWeekday> {
        vec![
            ScheduledWeekday::Monday,
            ScheduledWeekday::Tuesday,
            ScheduledWeekday::Wednesday,
            ScheduledWeekday::Thursday,
            ScheduledWeekday::Friday,
        ]
    }

    #[test]
    fn daily_schedule_uses_selected_local_weekdays() {
        let schedule = ScheduledTaskSchedule::Daily {
            time: "09:00".to_owned(),
            weekdays: weekdays(),
            timezone: "Asia/Singapore".to_owned(),
        };
        let friday_after_run = Utc.with_ymd_and_hms(2026, 7, 17, 2, 0, 0).unwrap();

        assert_eq!(
            next_daily_after(&schedule, friday_after_run).unwrap(),
            Utc.with_ymd_and_hms(2026, 7, 20, 1, 0, 0).unwrap()
        );
    }

    #[test]
    fn daily_schedule_skips_a_nonexistent_dst_wall_time() {
        let schedule = ScheduledTaskSchedule::Daily {
            time: "02:30".to_owned(),
            weekdays: vec![ScheduledWeekday::Sunday],
            timezone: "America/New_York".to_owned(),
        };
        let before_spring_forward = Utc.with_ymd_and_hms(2026, 3, 8, 6, 0, 0).unwrap();

        assert_eq!(
            next_daily_after(&schedule, before_spring_forward).unwrap(),
            Utc.with_ymd_and_hms(2026, 3, 15, 6, 30, 0).unwrap()
        );
    }

    #[test]
    fn interval_advancement_does_not_replay_every_missed_tick() {
        let schedule = ScheduledTaskSchedule::Interval {
            every: 15,
            unit: ScheduledIntervalUnit::Minutes,
        };
        let scheduled_for = Utc.with_ymd_and_hms(2026, 7, 17, 1, 0, 0).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 7, 17, 2, 2, 0).unwrap();

        assert_eq!(
            advance_past(&schedule, scheduled_for, now).unwrap(),
            Utc.with_ymd_and_hms(2026, 7, 17, 2, 15, 0).unwrap()
        );
    }

    #[test]
    fn schedule_validation_normalizes_weekdays_and_rejects_bad_timezones() {
        let normalized = normalize_schedule(ScheduledTaskSchedule::Daily {
            time: "09:00".to_owned(),
            weekdays: vec![
                ScheduledWeekday::Friday,
                ScheduledWeekday::Monday,
                ScheduledWeekday::Friday,
            ],
            timezone: "Asia/Singapore".to_owned(),
        })
        .unwrap();
        assert_eq!(
            normalized,
            ScheduledTaskSchedule::Daily {
                time: "09:00".to_owned(),
                weekdays: vec![ScheduledWeekday::Monday, ScheduledWeekday::Friday],
                timezone: "Asia/Singapore".to_owned(),
            }
        );

        assert!(matches!(
            normalize_schedule(ScheduledTaskSchedule::Daily {
                time: "09:00".to_owned(),
                weekdays: vec![ScheduledWeekday::Monday],
                timezone: "not/a-zone".to_owned(),
            }),
            Err(ScheduledTaskError::InvalidField {
                field: "schedule.timezone",
                ..
            })
        ));
    }
}
