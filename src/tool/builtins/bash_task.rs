use std::{
    collections::HashMap,
    future::Future,
    io,
    path::PathBuf,
    sync::{Arc, Mutex, Weak},
    time::Instant,
};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use tokio::{
    fs::{File, OpenOptions},
    sync::{OnceCell, oneshot},
    task::JoinHandle,
};

use super::{common::invalid_arguments, truncate::truncate_tail};
use crate::{
    error::ToolError,
    tool::{Tool, ToolEffect, ToolExecutionContext, ToolOutput},
    types::ToolDefinition,
};

const DEFAULT_MAX_RETAINED_TASKS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BashTaskStatus {
    Running,
    Completed,
    Failed,
    Stopped,
}

impl BashTaskStatus {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Stopped => "stopped",
        }
    }

    pub(super) fn is_terminal(self) -> bool {
        self != Self::Running
    }
}

struct BashTaskEntry {
    status: BashTaskStatus,
    description: String,
    output_path: PathBuf,
    started_at: Instant,
    finished_at: Option<Instant>,
    handle: Option<JoinHandle<()>>,
    preview: Vec<u8>,
    preview_bytes_dropped: usize,
    result: Option<String>,
    exit_code: Option<i32>,
    timed_out: bool,
    notification_error: Option<String>,
}

impl BashTaskEntry {
    fn running(description: String, output_path: PathBuf, handle: JoinHandle<()>) -> Self {
        Self {
            status: BashTaskStatus::Running,
            description,
            output_path,
            started_at: Instant::now(),
            finished_at: None,
            handle: Some(handle),
            preview: Vec::new(),
            preview_bytes_dropped: 0,
            result: None,
            exit_code: None,
            timed_out: false,
            notification_error: None,
        }
    }
}

struct BashTaskRegistryInner {
    tasks: HashMap<String, BashTaskEntry>,
    output_dir: PathBuf,
}

impl Drop for BashTaskRegistryInner {
    fn drop(&mut self) {
        for entry in self.tasks.values_mut() {
            if let Some(handle) = entry.handle.take() {
                handle.abort();
            }
            let _ = std::fs::remove_file(&entry.output_path);
        }
        let _ = std::fs::remove_dir(&self.output_dir);
    }
}

#[derive(Clone)]
pub(super) struct WeakBashTaskRegistry {
    inner: Weak<Mutex<BashTaskRegistryInner>>,
    preview_capacity: usize,
}

impl WeakBashTaskRegistry {
    pub(super) fn append_output(&self, task_id: &str, chunk: &[u8]) {
        if chunk.is_empty() {
            return;
        }
        let Some(shared) = self.inner.upgrade() else {
            return;
        };
        let mut inner = shared.lock().expect("bash task registry mutex poisoned");
        let Some(entry) = inner.tasks.get_mut(task_id) else {
            return;
        };
        entry.preview.extend_from_slice(chunk);
        if entry.preview.len() > self.preview_capacity {
            let drop_count = entry.preview.len() - self.preview_capacity;
            entry.preview.drain(..drop_count);
            entry.preview_bytes_dropped = entry.preview_bytes_dropped.saturating_add(drop_count);
        }
    }

    fn finish(
        &self,
        task_id: &str,
        result: Result<ToolOutput, ToolError>,
        notification_context: Option<ToolExecutionContext>,
    ) {
        let Some(shared) = self.inner.upgrade() else {
            return;
        };
        let mut inner = shared.lock().expect("bash task registry mutex poisoned");
        let Some(entry) = inner.tasks.get_mut(task_id) else {
            return;
        };
        // `bash_task_stop` wins a completion race once it records `stopped`.
        if entry.status != BashTaskStatus::Running {
            return;
        }
        entry.finished_at = Some(Instant::now());
        entry.handle = None;
        match result {
            Ok(output) => {
                entry.status = BashTaskStatus::Completed;
                entry.result = Some(output.content);
                entry.exit_code = Some(0);
            }
            Err(error) => {
                entry.status = BashTaskStatus::Failed;
                let result = error.to_string();
                entry.exit_code = parse_exit_code(&result);
                entry.timed_out = result.contains("Command timed out after ");
                entry.result = Some(result);
            }
        }
        let notification = terminal_notification(task_id, entry);
        drop(inner);
        let Some(context) = notification_context else {
            return;
        };
        if let Err(error) = context.notify_agent(notification) {
            let mut inner = shared.lock().expect("bash task registry mutex poisoned");
            if let Some(entry) = inner.tasks.get_mut(task_id) {
                entry.notification_error = Some(error.to_string());
            }
        }
    }
}

/// Shared state behind `bash` and `bash_task_stop`.
///
/// Entries are bounded. Finished tasks are evicted oldest-first when a new
/// task starts; running tasks are never silently evicted.
#[derive(Debug)]
pub(super) struct BashTaskStart {
    pub(super) task_id: String,
    pub(super) output_path: PathBuf,
}

#[derive(Clone)]
pub(super) struct BashTaskRegistry {
    inner: Arc<Mutex<BashTaskRegistryInner>>,
    output_dir_ready: Arc<OnceCell<()>>,
    max_tasks: usize,
    max_lines: usize,
    max_bytes: usize,
    preview_capacity: usize,
}

impl std::fmt::Debug for BashTaskRegistry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BashTaskRegistry")
            .field("max_tasks", &self.max_tasks)
            .field("max_lines", &self.max_lines)
            .field("max_bytes", &self.max_bytes)
            .field("output_dir", &self.output_root())
            .finish_non_exhaustive()
    }
}

impl BashTaskRegistry {
    pub(super) fn new(max_lines: usize, max_bytes: usize) -> Self {
        Self::build(max_lines, max_bytes, DEFAULT_MAX_RETAINED_TASKS)
    }

    #[cfg(test)]
    fn with_capacity(max_lines: usize, max_bytes: usize, max_tasks: usize) -> Self {
        Self::build(max_lines, max_bytes, max_tasks)
    }

    fn build(max_lines: usize, max_bytes: usize, max_tasks: usize) -> Self {
        let max_bytes = max_bytes.max(1);
        let output_dir = std::env::temp_dir().join(format!(
            "phi-tasks-{:016x}{:016x}",
            fastrand::u64(..),
            fastrand::u64(..)
        ));
        Self {
            inner: Arc::new(Mutex::new(BashTaskRegistryInner {
                tasks: HashMap::new(),
                output_dir,
            })),
            output_dir_ready: Arc::new(OnceCell::new()),
            max_tasks: max_tasks.max(1),
            max_lines: max_lines.max(1),
            max_bytes,
            preview_capacity: max_bytes.saturating_mul(2).max(8 * 1_024),
        }
    }

    pub(super) fn downgrade(&self) -> WeakBashTaskRegistry {
        WeakBashTaskRegistry {
            inner: Arc::downgrade(&self.inner),
            preview_capacity: self.preview_capacity,
        }
    }

    pub(super) fn output_root(&self) -> PathBuf {
        self.inner
            .lock()
            .expect("bash task registry mutex poisoned")
            .output_dir
            .clone()
    }

    async fn ensure_output_dir(&self) -> Result<(), ToolError> {
        let output_dir = self.output_root();
        self.output_dir_ready
            .get_or_try_init(|| async {
                let mut builder = tokio::fs::DirBuilder::new();
                #[cfg(unix)]
                {
                    builder.mode(0o700);
                }
                builder.create(&output_dir).await?;
                Ok::<(), io::Error>(())
            })
            .await
            .map(|_| ())
            .map_err(|error| {
                ToolError::new(format!(
                    "could not create background task output directory {}: {error}",
                    output_dir.display()
                ))
            })
    }

    async fn allocate_output_file(&self) -> Result<(String, PathBuf, File), ToolError> {
        self.ensure_output_dir().await?;
        let output_dir = self.output_root();
        for _ in 0..32 {
            let task_id = format!("bash_{:016x}", fastrand::u64(..));
            if self
                .inner
                .lock()
                .expect("bash task registry mutex poisoned")
                .tasks
                .contains_key(&task_id)
            {
                continue;
            }
            let output_path = output_dir.join(format!("{task_id}.output"));
            let mut options = OpenOptions::new();
            options.create_new(true).write(true);
            #[cfg(unix)]
            options.mode(0o600);
            match options.open(&output_path).await {
                Ok(file) => return Ok((task_id, output_path, file)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(ToolError::new(format!(
                        "could not create background task output file {}: {error}",
                        output_path.display()
                    )));
                }
            }
        }
        Err(ToolError::new(
            "could not allocate a unique background bash task id",
        ))
    }

    /// Starts a registered task behind a gate so it cannot produce output
    /// before its entry and abort handle are visible to the management tools.
    pub(super) async fn spawn<F, Fut>(
        &self,
        description: String,
        notification_context: Option<ToolExecutionContext>,
        run: F,
    ) -> Result<BashTaskStart, ToolError>
    where
        F: FnOnce(String, PathBuf, File) -> Fut + Send + 'static,
        Fut: Future<Output = Result<ToolOutput, ToolError>> + Send + 'static,
    {
        let evicted = {
            let mut inner = self
                .inner
                .lock()
                .expect("bash task registry mutex poisoned");
            let evicted = Self::evict_finished(&mut inner, self.max_tasks.saturating_sub(1));
            if inner.tasks.len() >= self.max_tasks {
                return Err(ToolError::new(format!(
                    "cannot start background bash task: {} tasks are still running",
                    self.max_tasks
                )));
            }
            evicted
        };
        for path in evicted {
            let _ = tokio::fs::remove_file(path).await;
        }

        let (task_id, output_path, output_file) = self.allocate_output_file().await?;
        let (start_sender, start_receiver) = oneshot::channel();
        let registry = self.downgrade();
        let task_id_for_run = task_id.clone();
        let output_path_for_run = output_path.clone();
        let handle = tokio::spawn(async move {
            if start_receiver.await.is_err() {
                return;
            }
            let result = run(task_id_for_run.clone(), output_path_for_run, output_file).await;
            registry.finish(&task_id_for_run, result, notification_context);
        });

        let mut handle = Some(handle);
        let inserted = {
            let mut inner = self
                .inner
                .lock()
                .expect("bash task registry mutex poisoned");
            if inner.tasks.len() >= self.max_tasks {
                false
            } else {
                inner.tasks.insert(
                    task_id.clone(),
                    BashTaskEntry::running(
                        description,
                        output_path.clone(),
                        handle.take().expect("background task handle is available"),
                    ),
                );
                true
            }
        };
        if !inserted {
            let mut handle = handle
                .take()
                .expect("unregistered background task handle is available");
            handle.abort();
            let _ = (&mut handle).await;
            let _ = tokio::fs::remove_file(&output_path).await;
            return Err(ToolError::new(format!(
                "cannot start background bash task: {} tasks are still running",
                self.max_tasks
            )));
        }
        // A closed receiver would mean the task was externally aborted in the
        // tiny interval after insertion. The entry remains queryable/stoppable.
        let _ = start_sender.send(());
        Ok(BashTaskStart {
            task_id,
            output_path,
        })
    }

    fn evict_finished(inner: &mut BashTaskRegistryInner, target_len: usize) -> Vec<PathBuf> {
        let mut output_paths = Vec::new();
        while inner.tasks.len() > target_len {
            let oldest = inner
                .tasks
                .iter()
                .filter(|(_, entry)| entry.status.is_terminal())
                .min_by_key(|(_, entry)| entry.finished_at.unwrap_or(entry.started_at))
                .map(|(task_id, _)| task_id.clone());
            let Some(oldest) = oldest else {
                break;
            };
            if let Some(entry) = inner.tasks.remove(&oldest) {
                output_paths.push(entry.output_path);
            }
        }
        output_paths
    }

    pub(super) fn snapshot(&self, task_id: &str) -> Result<BashTaskSnapshot, ToolError> {
        let inner = self
            .inner
            .lock()
            .expect("bash task registry mutex poisoned");
        let entry = inner.tasks.get(task_id).ok_or_else(|| {
            ToolError::new(format!(
                "unknown or expired background bash task: {task_id}"
            ))
        })?;
        let output = entry
            .result
            .clone()
            .unwrap_or_else(|| self.format_live_output(entry));
        Ok(BashTaskSnapshot {
            task_id: task_id.to_owned(),
            description: entry.description.clone(),
            status: entry.status,
            output_path: entry.output_path.clone(),
            output,
            elapsed_seconds: entry
                .finished_at
                .map_or_else(
                    || entry.started_at.elapsed(),
                    |finished_at| finished_at.duration_since(entry.started_at),
                )
                .as_secs_f64(),
            exit_code: entry.exit_code,
            timed_out: entry.timed_out,
            notification_error: entry.notification_error.clone(),
        })
    }

    fn format_live_output(&self, entry: &BashTaskEntry) -> String {
        let decoded = String::from_utf8_lossy(&entry.preview);
        let preview = truncate_tail(&decoded, self.max_lines, self.max_bytes);
        let mut output = preview.content;
        if entry.preview_bytes_dropped > 0 || preview.truncated {
            let omitted = entry
                .preview_bytes_dropped
                .saturating_add(decoded.len().saturating_sub(preview.output_bytes));
            output = append_note(
                output,
                &format!("[Live output truncated; at least {omitted} earlier bytes omitted]"),
            );
        }
        output
    }

    async fn stop(&self, task_id: &str) -> Result<BashTaskSnapshot, ToolError> {
        let handle = {
            let mut inner = self
                .inner
                .lock()
                .expect("bash task registry mutex poisoned");
            let entry = inner.tasks.get_mut(task_id).ok_or_else(|| {
                ToolError::new(format!(
                    "unknown or expired background bash task: {task_id}"
                ))
            })?;
            if entry.status == BashTaskStatus::Running {
                entry.status = BashTaskStatus::Stopped;
                entry.finished_at = Some(Instant::now());
                entry.result = Some(append_note(
                    self.format_live_output(entry),
                    "Background bash task was stopped",
                ));
                entry.handle.take()
            } else {
                None
            }
        };
        // Awaiting the aborted task drops Bash's ProcessGroupGuard and
        // synchronously terminates the shell process group before returning.
        if let Some(handle) = handle {
            handle.abort();
            let _ = handle.await;
        }
        self.snapshot(task_id)
    }
}

fn append_note(text: String, note: &str) -> String {
    if text.is_empty() {
        note.to_owned()
    } else {
        format!("{text}\n\n{note}")
    }
}

fn parse_exit_code(output: &str) -> Option<i32> {
    output
        .rsplit_once("Command exited with code ")
        .and_then(|(_, code)| code.trim().parse().ok())
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn terminal_notification(task_id: &str, entry: &BashTaskEntry) -> String {
    let status = entry.status.as_str();
    let summary = match entry.status {
        BashTaskStatus::Completed => format!(
            "Background command \"{}\" completed successfully",
            entry.description
        ),
        BashTaskStatus::Failed if entry.timed_out => {
            format!("Background command \"{}\" timed out", entry.description)
        }
        BashTaskStatus::Failed => entry.exit_code.map_or_else(
            || format!("Background command \"{}\" failed", entry.description),
            |exit_code| {
                format!(
                    "Background command \"{}\" failed with exit code {exit_code}",
                    entry.description
                )
            },
        ),
        BashTaskStatus::Stopped => {
            format!("Background command \"{}\" was stopped", entry.description)
        }
        BashTaskStatus::Running => {
            format!(
                "Background command \"{}\" is still running",
                entry.description
            )
        }
    };
    let exit_code = entry.exit_code.map_or_else(String::new, |exit_code| {
        format!("\n<exit_code>{exit_code}</exit_code>")
    });
    format!(
        "<task_notification>\n<task_id>{}</task_id>\n<task_type>local_bash</task_type>\n<output_file>{}</output_file>\n<status>{status}</status>{exit_code}\n<summary>{}</summary>\n</task_notification>",
        escape_xml(task_id),
        escape_xml(&entry.output_path.display().to_string()),
        escape_xml(&summary),
    )
}

pub(super) struct BashTaskSnapshot {
    task_id: String,
    description: String,
    pub(super) status: BashTaskStatus,
    pub(super) output_path: PathBuf,
    pub(super) output: String,
    elapsed_seconds: f64,
    pub(super) exit_code: Option<i32>,
    pub(super) timed_out: bool,
    notification_error: Option<String>,
}

impl BashTaskSnapshot {
    fn into_stop_output(self) -> ToolOutput {
        let retrieval_status = "success";
        let status = self.status.as_str();
        let output_path = self.output_path.display().to_string();
        let mut parts = vec![
            format!("<retrieval_status>{retrieval_status}</retrieval_status>"),
            format!("<task_id>{}</task_id>", escape_xml(&self.task_id)),
            "<task_type>local_bash</task_type>".to_owned(),
            format!("<status>{status}</status>"),
            format!("<output_file>{}</output_file>", escape_xml(&output_path)),
        ];
        if let Some(exit_code) = self.exit_code {
            parts.push(format!("<exit_code>{exit_code}</exit_code>"));
        }
        if !self.output.trim().is_empty() {
            parts.push(format!("<output>\n{}\n</output>", self.output.trim_end()));
        }
        if let Some(error) = &self.notification_error {
            parts.push(format!(
                "<notification_error>{}</notification_error>",
                escape_xml(error)
            ));
        }
        let mut metadata = json!({
            "task_id": self.task_id,
            "task_type": "local_bash",
            "description": self.description,
            "status": status,
            "retrieval_status": retrieval_status,
            "output_file": output_path,
            "elapsed_seconds": self.elapsed_seconds,
            "timed_out": self.timed_out,
        });
        if let Some(exit_code) = self.exit_code {
            metadata["exit_code"] = json!(exit_code);
        }
        if let Some(error) = self.notification_error {
            metadata["notification_error"] = json!(error);
        }
        ToolOutput::success(parts.join("\n\n")).with_metadata(metadata)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskArguments {
    task_id: String,
}

#[derive(Clone, Debug)]
pub(super) struct BashTaskStopTool {
    registry: BashTaskRegistry,
}

impl BashTaskStopTool {
    pub(super) fn new(registry: BashTaskRegistry) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl Tool for BashTaskStopTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "bash_task_stop",
            "Stop a running background Bash task and its process group.",
            json!({
                "type": "object",
                "properties": {
                    "task_id": {
                        "type": "string",
                        "description": "Task id returned by bash with run_in_background=true"
                    }
                },
                "required": ["task_id"],
                "additionalProperties": false
            }),
        )
    }

    fn effect(&self) -> ToolEffect {
        ToolEffect::ExternalSideEffect
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, ToolError> {
        let arguments: TaskArguments = serde_json::from_value(arguments)
            .map_err(|error| invalid_arguments("bash_task_stop", error))?;
        self.registry
            .stop(&arguments.task_id)
            .await
            .map(BashTaskSnapshot::into_stop_output)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn registry_evicts_oldest_finished_task() {
        let registry = BashTaskRegistry::with_capacity(20, 1_024, 1);
        let first = registry
            .spawn("first".to_owned(), None, |_, _, _| async {
                Ok(ToolOutput::success("first"))
            })
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(
            registry.snapshot(&first.task_id).unwrap().status,
            BashTaskStatus::Completed
        );

        let second = registry
            .spawn("second".to_owned(), None, |_, _, _| async {
                Ok(ToolOutput::success("second"))
            })
            .await
            .unwrap();
        assert!(registry.snapshot(&first.task_id).is_err());
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(
            registry.snapshot(&second.task_id).unwrap().status,
            BashTaskStatus::Completed
        );
    }

    #[tokio::test]
    async fn registry_refuses_to_evict_running_task() {
        let registry = BashTaskRegistry::with_capacity(20, 1_024, 1);
        let _task = registry
            .spawn("pending".to_owned(), None, |_, _, _| async {
                std::future::pending::<()>().await;
                Ok(ToolOutput::success("unreachable"))
            })
            .await
            .unwrap();
        let error = registry
            .spawn("second".to_owned(), None, |_, _, _| async {
                Ok(ToolOutput::success("second"))
            })
            .await
            .unwrap_err();
        assert!(error.to_string().contains("still running"));
    }
}
