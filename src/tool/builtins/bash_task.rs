use std::{
    collections::HashMap,
    future::Future,
    sync::{Arc, Mutex, Weak},
    time::Instant,
};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use tokio::{sync::oneshot, task::JoinHandle};

use super::{common::invalid_arguments, truncate::truncate_tail};
use crate::{
    error::ToolError,
    tool::{Tool, ToolEffect, ToolOutput},
    types::ToolDefinition,
};

const DEFAULT_MAX_RETAINED_TASKS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BashTaskStatus {
    Running,
    Completed,
    Failed,
    Stopped,
}

impl BashTaskStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Stopped => "stopped",
        }
    }

    fn is_terminal(self) -> bool {
        self != Self::Running
    }
}

struct BashTaskEntry {
    status: BashTaskStatus,
    started_at: Instant,
    finished_at: Option<Instant>,
    handle: Option<JoinHandle<()>>,
    preview: Vec<u8>,
    preview_bytes_dropped: usize,
    result: Option<String>,
    exit_code: Option<i32>,
    timed_out: bool,
}

impl BashTaskEntry {
    fn running(handle: JoinHandle<()>) -> Self {
        Self {
            status: BashTaskStatus::Running,
            started_at: Instant::now(),
            finished_at: None,
            handle: Some(handle),
            preview: Vec::new(),
            preview_bytes_dropped: 0,
            result: None,
            exit_code: None,
            timed_out: false,
        }
    }
}

struct BashTaskRegistryInner {
    tasks: HashMap<String, BashTaskEntry>,
}

impl Drop for BashTaskRegistryInner {
    fn drop(&mut self) {
        for entry in self.tasks.values_mut() {
            if let Some(handle) = entry.handle.take() {
                handle.abort();
            }
        }
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
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        let mut inner = inner.lock().expect("bash task registry mutex poisoned");
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

    fn finish(&self, task_id: &str, result: Result<ToolOutput, ToolError>) {
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        let mut inner = inner.lock().expect("bash task registry mutex poisoned");
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
    }
}

/// Shared state behind `bash`, `bash_task_output`, and `bash_task_stop`.
///
/// Entries are bounded. Finished tasks are evicted oldest-first when a new
/// task starts; running tasks are never silently evicted.
#[derive(Clone)]
pub(super) struct BashTaskRegistry {
    inner: Arc<Mutex<BashTaskRegistryInner>>,
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
        Self {
            inner: Arc::new(Mutex::new(BashTaskRegistryInner {
                tasks: HashMap::new(),
            })),
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

    /// Starts a registered task behind a gate so it cannot produce output
    /// before its entry and abort handle are visible to the management tools.
    pub(super) fn spawn<F, Fut>(&self, run: F) -> Result<String, ToolError>
    where
        F: FnOnce(String) -> Fut + Send + 'static,
        Fut: Future<Output = Result<ToolOutput, ToolError>> + Send + 'static,
    {
        let mut inner = self
            .inner
            .lock()
            .expect("bash task registry mutex poisoned");
        Self::evict_finished(&mut inner, self.max_tasks.saturating_sub(1));
        if inner.tasks.len() >= self.max_tasks {
            return Err(ToolError::new(format!(
                "cannot start background bash task: {} tasks are still running",
                self.max_tasks
            )));
        }
        let task_id = (0..32)
            .map(|_| format!("bash_{:016x}", fastrand::u64(..)))
            .find(|candidate| !inner.tasks.contains_key(candidate))
            .ok_or_else(|| ToolError::new("could not allocate a unique background bash task id"))?;
        let (start_sender, start_receiver) = oneshot::channel();
        let registry = self.downgrade();
        let task_id_for_run = task_id.clone();
        let handle = tokio::spawn(async move {
            if start_receiver.await.is_err() {
                return;
            }
            let result = run(task_id_for_run.clone()).await;
            registry.finish(&task_id_for_run, result);
        });

        inner
            .tasks
            .insert(task_id.clone(), BashTaskEntry::running(handle));
        drop(inner);
        // A closed receiver would mean the task was externally aborted in the
        // tiny interval after insertion. The entry remains queryable/stoppable.
        let _ = start_sender.send(());
        Ok(task_id)
    }

    fn evict_finished(inner: &mut BashTaskRegistryInner, target_len: usize) {
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
            inner.tasks.remove(&oldest);
        }
    }

    fn snapshot(&self, task_id: &str) -> Result<BashTaskSnapshot, ToolError> {
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
            status: entry.status,
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

struct BashTaskSnapshot {
    task_id: String,
    status: BashTaskStatus,
    output: String,
    elapsed_seconds: f64,
    exit_code: Option<i32>,
    timed_out: bool,
}

impl BashTaskSnapshot {
    fn into_output(self) -> ToolOutput {
        let status = self.status.as_str();
        let content = if self.output.is_empty() {
            format!(
                "Background bash task {} is {status} (no output yet)",
                self.task_id
            )
        } else {
            self.output
        };
        let mut metadata = json!({
            "task_id": self.task_id,
            "status": status,
            "elapsed_seconds": self.elapsed_seconds,
            "timed_out": self.timed_out,
        });
        if let Some(exit_code) = self.exit_code {
            metadata["exit_code"] = json!(exit_code);
        }
        ToolOutput::success(content).with_metadata(metadata)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskArguments {
    task_id: String,
}

#[derive(Clone, Debug)]
pub(super) struct BashTaskOutputTool {
    registry: BashTaskRegistry,
}

impl BashTaskOutputTool {
    pub(super) fn new(registry: BashTaskRegistry) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl Tool for BashTaskOutputTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "bash_task_output",
            "Get the current status and available output of a background Bash task.",
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
        ToolEffect::ReadOnly
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, ToolError> {
        let arguments: TaskArguments = serde_json::from_value(arguments)
            .map_err(|error| invalid_arguments("bash_task_output", error))?;
        self.registry
            .snapshot(&arguments.task_id)
            .map(BashTaskSnapshot::into_output)
    }
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
            .map(BashTaskSnapshot::into_output)
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
            .spawn(|_| async { Ok(ToolOutput::success("first")) })
            .unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(
            registry.snapshot(&first).unwrap().status,
            BashTaskStatus::Completed
        );

        let second = registry
            .spawn(|_| async { Ok(ToolOutput::success("second")) })
            .unwrap();
        assert!(registry.snapshot(&first).is_err());
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(
            registry.snapshot(&second).unwrap().status,
            BashTaskStatus::Completed
        );
    }

    #[tokio::test]
    async fn registry_refuses_to_evict_running_task() {
        let registry = BashTaskRegistry::with_capacity(20, 1_024, 1);
        let _task = registry
            .spawn(|_| async {
                std::future::pending::<()>().await;
                Ok(ToolOutput::success("unreachable"))
            })
            .unwrap();
        let error = registry
            .spawn(|_| async { Ok(ToolOutput::success("second")) })
            .unwrap_err();
        assert!(error.to_string().contains("still running"));
    }
}
