use std::{
    io,
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use tokio::{
    fs::{File, OpenOptions},
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::{Child, Command},
    sync::mpsc,
    task::JoinHandle,
};

use super::{
    bash_classifier::classify_bash_arguments_concurrency,
    bash_task::BashTaskRegistry,
    common::{invalid_arguments, normalize_cwd},
    truncate::{DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES, TruncatedBy, truncate_tail},
};
use crate::{
    error::ToolError,
    tool::{Tool, ToolConcurrency, ToolEffect, ToolExecutionContext, ToolOutput, ToolProgress},
    types::ToolDefinition,
};

const MAX_TIMEOUT_SECONDS: f64 = 2_147_483_647_f64 / 1_000_f64;
const MAX_BASH_OUTPUT_BYTES: u64 = 5 * 1_024 * 1_024 * 1_024;
pub const DEFAULT_BASH_TIMEOUT: Duration = Duration::from_secs(120);
#[cfg(not(test))]
const OUTPUT_DRAIN_GRACE: Duration = Duration::from_secs(1);
#[cfg(test)]
const OUTPUT_DRAIN_GRACE: Duration = Duration::from_millis(100);

#[derive(Clone, Debug)]
pub struct BashTool {
    cwd: PathBuf,
    shell: PathBuf,
    max_lines: usize,
    max_bytes: usize,
    default_timeout: Option<Duration>,
    task_registry: BashTaskRegistry,
}

impl BashTool {
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self {
            cwd: normalize_cwd(cwd),
            shell: default_shell(),
            max_lines: DEFAULT_MAX_LINES,
            max_bytes: DEFAULT_MAX_BYTES,
            default_timeout: Some(DEFAULT_BASH_TIMEOUT),
            task_registry: BashTaskRegistry::new(DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES),
        }
    }

    pub fn shell(mut self, shell: impl Into<PathBuf>) -> Self {
        self.shell = shell.into();
        self
    }

    pub fn output_limits(mut self, max_lines: usize, max_bytes: usize) -> Self {
        self.max_lines = max_lines.max(1);
        self.max_bytes = max_bytes.max(1);
        self.task_registry = BashTaskRegistry::new(self.max_lines, self.max_bytes);
        self
    }

    pub(super) fn task_registry(mut self, task_registry: BashTaskRegistry) -> Self {
        self.task_registry = task_registry;
        self
    }

    /// Sets the timeout used when a call omits its `timeout` argument.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = Some(nonzero_timeout(timeout));
        self
    }

    /// Disables the Bash-specific default timeout. An explicit call argument
    /// and the agent-level tool-call timeout may still apply.
    pub fn without_timeout(mut self) -> Self {
        self.default_timeout = None;
        self
    }

    pub fn configured_timeout(&self) -> Option<Duration> {
        self.default_timeout
    }

    /// Changes the timeout used by subsequent calls to this tool.
    pub fn set_timeout(&mut self, timeout: Option<Duration>) {
        self.default_timeout = timeout.map(nonzero_timeout);
    }

    async fn validate_cwd(&self) -> Result<(), ToolError> {
        let metadata = tokio::fs::metadata(&self.cwd).await.map_err(|error| {
            ToolError::new(format!(
                "working directory does not exist: {}: {error}",
                self.cwd.display()
            ))
        })?;
        if !metadata.is_dir() {
            return Err(ToolError::new(format!(
                "working directory is not a directory: {}",
                self.cwd.display()
            )));
        }
        Ok(())
    }

    async fn start_background(
        &self,
        arguments: ResolvedBashArguments,
        description: String,
        notification_context: Option<ToolExecutionContext>,
    ) -> Result<ToolOutput, ToolError> {
        // The running task must not retain its management registry. Otherwise
        // the registry owns the JoinHandle while the task owns the registry,
        // preventing Agent drop from cancelling an orphaned command.
        let tool = Self {
            task_registry: BashTaskRegistry::new(self.max_lines, self.max_bytes),
            ..self.clone()
        };
        let task_registry = self.task_registry.clone();
        let registry_for_run = task_registry.downgrade();
        let completion_notification = notification_context
            .as_ref()
            .is_some_and(ToolExecutionContext::can_notify_agent);
        let notification_context =
            notification_context.filter(|context| context.can_notify_agent());
        let started = task_registry
            .spawn(
                description.clone(),
                notification_context,
                move |task_id, output_path, output_file| {
                    let registry_for_output = registry_for_run.clone();
                    let output_task_id = task_id.clone();
                    let observer: OutputObserver = Arc::new(move |chunk| {
                        registry_for_output.append_output(&output_task_id, chunk);
                    });
                    async move {
                        tool.run_foreground(
                            arguments,
                            Some(observer),
                            Some((output_file, output_path)),
                        )
                        .await
                    }
                },
            )
            .await?;
        let notification_note = if completion_notification {
            "You will receive a task_notification when it finishes. Do not poll; use read on output_file after that notification."
        } else {
            "This host has no automatic completion mailbox. Use the deprecated bash_task_output compatibility tool if you must wait for completion."
        };
        Ok(ToolOutput::success(format!(
            "Background command running with task ID: {}\nOutput is being written to: {}\n{notification_note}\nUse bash_task_stop to stop it.",
            started.task_id,
            started.output_path.display(),
        ))
        .with_metadata(json!({
            "task_id": started.task_id,
            "status": "running",
            "task_type": "local_bash",
            "description": description,
            "output_file": started.output_path,
            "completion_notification": completion_notification,
        })))
    }

    async fn run_foreground(
        &self,
        arguments: ResolvedBashArguments,
        observer: Option<OutputObserver>,
        persisted_output: Option<(File, PathBuf)>,
    ) -> Result<ToolOutput, ToolError> {
        self.validate_cwd().await?;

        let mut command = shell_command(&self.shell, &arguments.command);
        command
            .current_dir(&self.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        configure_process_group(&mut command);
        let mut child = command.spawn().map_err(|error| {
            ToolError::new(format!(
                "could not execute shell {}: {error}",
                self.shell.display()
            ))
        })?;
        let mut process_guard = ProcessGroupGuard::new(child.id());

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ToolError::new("could not capture command stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| ToolError::new("could not capture command stderr"))?;
        let (sender, receiver) = mpsc::channel(16);
        let stdout_task = tokio::spawn(pump_output(stdout, sender.clone()));
        let stderr_task = tokio::spawn(pump_output(stderr, sender.clone()));
        drop(sender);
        let max_lines = self.max_lines;
        let max_bytes = self.max_bytes;
        let collector = tokio::spawn(async move {
            let mut accumulator = persisted_output.map_or_else(
                || BashOutputAccumulator::new(max_lines, max_bytes),
                |(file, path)| {
                    BashOutputAccumulator::with_full_output(max_lines, max_bytes, file, path)
                },
            );
            collect_output(receiver, &mut accumulator, observer.as_ref()).await?;
            accumulator.finish().await
        });

        let wait_outcome = wait_for_child(&mut child, arguments.timeout).await;
        let (snapshot, drain_timed_out) =
            drain_output_tasks(stdout_task, stderr_task, collector, &process_guard).await?;
        process_guard.disarm();
        let drain_note = drain_timed_out.then(|| {
            format!(
                "Output drain exceeded {:.3} seconds after the command exited; remaining process-group members and output tasks were terminated. Output may be incomplete.",
                OUTPUT_DRAIN_GRACE.as_secs_f64()
            )
        });

        match wait_outcome {
            WaitOutcome::Exited(status) => {
                let status = status.map_err(|error| {
                    ToolError::new(format!("failed waiting for command to exit: {error}"))
                })?;
                let output = drain_note.as_deref().map_or_else(
                    || format_output(&snapshot, if status.success() { "(no output)" } else { "" }),
                    |note| append_status(format_output(&snapshot, ""), note),
                );
                if status.success() {
                    Ok(ToolOutput::success(output))
                } else {
                    Err(ToolError::new(append_status(
                        output,
                        &format_exit_status(status),
                    )))
                }
            }
            WaitOutcome::TimedOut(duration) => {
                let output = drain_note.as_deref().map_or_else(
                    || format_output(&snapshot, ""),
                    |note| append_status(format_output(&snapshot, ""), note),
                );
                Err(ToolError::new(append_status(
                    output,
                    &format!("Command timed out after {} seconds", duration.as_secs_f64()),
                )))
            }
        }
    }

    async fn execute_inner(
        &self,
        arguments: serde_json::Value,
        notification_context: Option<ToolExecutionContext>,
    ) -> Result<ToolOutput, ToolError> {
        let arguments: BashArguments =
            serde_json::from_value(arguments).map_err(|error| invalid_arguments("bash", error))?;
        let description = task_description(arguments.description.as_deref(), &arguments.command);
        let resolved = ResolvedBashArguments {
            command: arguments.command,
            timeout: resolve_timeout(arguments.timeout, self.default_timeout)?,
        };
        if arguments.run_in_background {
            // Fail obvious configuration errors synchronously instead of
            // returning an id for a task that can never start.
            self.validate_cwd().await?;
            self.start_background(resolved, description, notification_context)
                .await
        } else {
            self.run_foreground(resolved, None, None).await
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BashArguments {
    command: String,
    description: Option<String>,
    timeout: Option<f64>,
    #[serde(default)]
    run_in_background: bool,
}

struct ResolvedBashArguments {
    command: String,
    timeout: Option<Duration>,
}

type OutputObserver = Arc<dyn Fn(&[u8]) + Send + Sync>;

#[async_trait]
impl Tool for BashTool {
    fn effect(&self) -> ToolEffect {
        ToolEffect::ExternalSideEffect
    }

    fn concurrency(&self, arguments: &serde_json::Value) -> ToolConcurrency {
        if classify_bash_arguments_concurrency(arguments) {
            ToolConcurrency::Safe
        } else {
            ToolConcurrency::Exclusive
        }
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "bash",
            format!(
                "Execute a shell command in the configured working directory. Returns combined stdout and stderr, truncated to the last {} lines or {} bytes. Truncated full output is saved to a temporary file. The optional timeout is measured in seconds and overrides the configured default{}. Set run_in_background=true only when the result is not needed immediately. A background call returns task_id and output_file immediately; when the Agent host supports notifications, wait for task_notification instead of polling, then use read on output_file. Use bash_task_stop to stop it. bash_task_output is a deprecated compatibility fallback.",
                self.max_lines,
                self.max_bytes,
                self.default_timeout.map_or_else(
                    || " (none)".to_owned(),
                    |timeout| format!(" ({:.3}s)", timeout.as_secs_f64())
                )
            ),
            json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "Shell command to execute"
                    },
                    "description": {
                        "type": "string",
                        "description": "Short active-voice description of what the command does"
                    },
                    "timeout": {
                        "type": "number",
                        "exclusiveMinimum": 0,
                        "description": "Optional timeout in seconds; overrides the configured Bash default"
                    },
                    "run_in_background": {
                        "type": "boolean",
                        "default": false,
                        "description": "Run as a managed background task, return task_id and output_file immediately, and use read after the completion notification"
                    }
                },
                "required": ["command"],
                "additionalProperties": false
            }),
        )
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, ToolError> {
        self.execute_inner(arguments, None).await
    }

    async fn execute_with_context(
        &self,
        arguments: serde_json::Value,
        context: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let background = arguments
            .get("run_in_background")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        context.report_progress(ToolProgress::new(if background {
            "Starting background command"
        } else {
            "Command started"
        }));
        let execution = self.execute_inner(arguments, Some(context.clone()));
        tokio::pin!(execution);
        let mut heartbeat = tokio::time::interval(Duration::from_secs(5));
        heartbeat.tick().await;
        let result = loop {
            tokio::select! {
                biased;
                result = &mut execution => break result,
                _ = context.cancelled() => {
                    break Err(ToolError::new("bash command cancelled"));
                }
                _ = heartbeat.tick() => {
                    context.report_progress(ToolProgress::new("Command is still running"));
                }
            }
        };
        context.report_progress(
            ToolProgress::new(match (background, result.is_ok()) {
                (true, true) => "Background task started",
                (true, false) => "Background task failed to start",
                (false, true) => "Command finished",
                (false, false) => "Command failed",
            })
            .with_metadata(json!({
                "is_error": result.is_err(),
                "background": background,
            })),
        );
        result
    }
}

fn task_description(description: Option<&str>, command: &str) -> String {
    let source = description
        .map(str::trim)
        .filter(|description| !description.is_empty())
        .unwrap_or_else(|| command.trim());
    let mut characters = source.chars();
    let mut concise = characters.by_ref().take(200).collect::<String>();
    if characters.next().is_some() {
        concise.push('…');
    }
    if concise.is_empty() {
        "background command".to_owned()
    } else {
        concise
    }
}

fn default_shell() -> PathBuf {
    #[cfg(windows)]
    {
        std::env::var_os("COMSPEC")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("cmd.exe"))
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("SHELL")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/bin/bash"))
    }
}

fn shell_command(shell: &Path, command: &str) -> Command {
    let mut process = Command::new(shell);
    #[cfg(windows)]
    process.args(["/C", command]);
    #[cfg(not(windows))]
    process.args(["-lc", command]);
    process
}

fn resolve_timeout(
    timeout: Option<f64>,
    default_timeout: Option<Duration>,
) -> Result<Option<Duration>, ToolError> {
    let Some(timeout) = timeout else {
        return Ok(default_timeout);
    };
    if !timeout.is_finite() || timeout <= 0.0 {
        return Err(ToolError::new(
            "invalid bash timeout: expected a finite positive number of seconds",
        ));
    }
    if timeout > MAX_TIMEOUT_SECONDS {
        return Err(ToolError::new(format!(
            "invalid bash timeout: maximum is {MAX_TIMEOUT_SECONDS} seconds"
        )));
    }
    Ok(Some(Duration::from_secs_f64(timeout)))
}

fn nonzero_timeout(timeout: Duration) -> Duration {
    timeout.max(Duration::from_millis(1))
}

struct ProcessGroupGuard {
    #[cfg(unix)]
    pid: Option<u32>,
}

impl ProcessGroupGuard {
    fn new(pid: Option<u32>) -> Self {
        Self {
            #[cfg(unix)]
            pid,
        }
    }

    fn disarm(&mut self) {
        #[cfg(unix)]
        {
            self.pid = None;
        }
    }

    fn terminate(&self) {
        #[cfg(unix)]
        if let Some(pid) = self.pid {
            // The shell is the process-group leader. It may already have
            // exited, but descendants that inherited the group can still be
            // holding stdout/stderr open.
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
    }
}

#[cfg(unix)]
impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        if self.pid.is_some() {
            // The shell is started as process-group leader. This synchronous
            // best-effort kill also runs when an outer agent timeout or stop
            // drops the Bash future, cleaning descendants still in the group.
            self.terminate();
        }
    }
}

enum WaitOutcome {
    Exited(io::Result<ExitStatus>),
    TimedOut(Duration),
}

async fn wait_for_child(child: &mut Child, timeout: Option<Duration>) -> WaitOutcome {
    match timeout {
        Some(duration) => match tokio::time::timeout(duration, child.wait()).await {
            Ok(status) => WaitOutcome::Exited(status),
            Err(_) => {
                terminate_child(child).await;
                WaitOutcome::TimedOut(duration)
            }
        },
        None => WaitOutcome::Exited(child.wait().await),
    }
}

#[cfg(unix)]
fn configure_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt as _;

    command.as_std_mut().process_group(0);
}

#[cfg(not(unix))]
fn configure_process_group(_command: &mut Command) {}

async fn terminate_child(child: &mut Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        // The child starts its own process group, so a negative PID terminates
        // the shell and descendants that inherited that group.
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
    let _ = child.kill().await;
    let _ = child.wait().await;
}

async fn pump_output<R>(mut reader: R, sender: mpsc::Sender<Vec<u8>>) -> io::Result<()>
where
    R: AsyncRead + Unpin,
{
    let mut buffer = vec![0; 8 * 1_024];
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            return Ok(());
        }
        if sender.send(buffer[..read].to_vec()).await.is_err() {
            return Ok(());
        }
    }
}

async fn drain_output_tasks(
    mut stdout_task: JoinHandle<io::Result<()>>,
    mut stderr_task: JoinHandle<io::Result<()>>,
    mut collector: JoinHandle<io::Result<BashOutputSnapshot>>,
    process_guard: &ProcessGroupGuard,
) -> Result<(BashOutputSnapshot, bool), ToolError> {
    let deadline = tokio::time::sleep(OUTPUT_DRAIN_GRACE);
    tokio::pin!(deadline);
    let mut stdout_result = None;
    let mut stderr_result = None;
    let mut snapshot_result = None;
    let mut timed_out = false;

    while stdout_result.is_none() || stderr_result.is_none() || snapshot_result.is_none() {
        tokio::select! {
            result = &mut stdout_task, if stdout_result.is_none() => {
                stdout_result = Some(map_reader_result(result, "stdout"));
            }
            result = &mut stderr_task, if stderr_result.is_none() => {
                stderr_result = Some(map_reader_result(result, "stderr"));
            }
            result = &mut collector, if snapshot_result.is_none() => {
                snapshot_result = Some(map_collector_result(result));
            }
            _ = &mut deadline => {
                timed_out = true;
                break;
            }
        }
    }

    if timed_out {
        process_guard.terminate();
        if stdout_result.is_none() {
            stdout_task.abort();
            let _ = (&mut stdout_task).await;
            stdout_result = Some(Ok(()));
        }
        if stderr_result.is_none() {
            stderr_task.abort();
            let _ = (&mut stderr_task).await;
            stderr_result = Some(Ok(()));
        }
        // Aborting the readers drops the last channel senders, allowing the
        // collector to flush and return everything received before the grace
        // deadline instead of losing the partial output.
        if snapshot_result.is_none() {
            snapshot_result = Some(map_collector_result(collector.await));
        }
    }

    stdout_result.expect("stdout task completed or was aborted")?;
    stderr_result.expect("stderr task completed or was aborted")?;
    let snapshot = snapshot_result.expect("collector completed after reader shutdown")?;
    Ok((snapshot, timed_out))
}

fn map_reader_result(
    result: Result<io::Result<()>, tokio::task::JoinError>,
    stream: &str,
) -> Result<(), ToolError> {
    result
        .map_err(|error| ToolError::new(format!("bash {stream} reader failed: {error}")))?
        .map_err(|error| ToolError::new(format!("could not read bash {stream}: {error}")))
}

fn map_collector_result(
    result: Result<io::Result<BashOutputSnapshot>, tokio::task::JoinError>,
) -> Result<BashOutputSnapshot, ToolError> {
    result
        .map_err(|error| ToolError::new(format!("bash output collector failed: {error}")))?
        .map_err(|error| ToolError::new(format!("could not collect bash output: {error}")))
}

async fn collect_output(
    mut receiver: mpsc::Receiver<Vec<u8>>,
    accumulator: &mut BashOutputAccumulator,
    observer: Option<&OutputObserver>,
) -> io::Result<()> {
    while let Some(chunk) = receiver.recv().await {
        if let Some(observer) = observer {
            observer(&chunk);
        }
        accumulator.append(&chunk).await?;
    }
    Ok(())
}

struct BashOutputAccumulator {
    max_lines: usize,
    max_bytes: usize,
    total_bytes: usize,
    newline_count: usize,
    has_output: bool,
    ends_with_newline: bool,
    prefix: Vec<u8>,
    tail: Vec<u8>,
    full_output: Option<File>,
    full_output_path: Option<PathBuf>,
    full_output_limit: u64,
    full_output_bytes_written: u64,
    full_output_capped: bool,
}

impl BashOutputAccumulator {
    fn new(max_lines: usize, max_bytes: usize) -> Self {
        Self {
            max_lines,
            max_bytes,
            total_bytes: 0,
            newline_count: 0,
            has_output: false,
            ends_with_newline: false,
            prefix: Vec::new(),
            tail: Vec::new(),
            full_output: None,
            full_output_path: None,
            full_output_limit: MAX_BASH_OUTPUT_BYTES,
            full_output_bytes_written: 0,
            full_output_capped: false,
        }
    }

    fn with_full_output(
        max_lines: usize,
        max_bytes: usize,
        full_output: File,
        full_output_path: PathBuf,
    ) -> Self {
        Self::with_full_output_limit(
            max_lines,
            max_bytes,
            full_output,
            full_output_path,
            MAX_BASH_OUTPUT_BYTES,
        )
    }

    fn with_full_output_limit(
        max_lines: usize,
        max_bytes: usize,
        full_output: File,
        full_output_path: PathBuf,
        full_output_limit: u64,
    ) -> Self {
        Self {
            full_output: Some(full_output),
            full_output_path: Some(full_output_path),
            full_output_limit,
            ..Self::new(max_lines, max_bytes)
        }
    }

    async fn append(&mut self, chunk: &[u8]) -> io::Result<()> {
        if chunk.is_empty() {
            return Ok(());
        }
        self.total_bytes = self.total_bytes.saturating_add(chunk.len());
        self.newline_count = self
            .newline_count
            .saturating_add(chunk.iter().filter(|byte| **byte == b'\n').count());
        self.has_output = true;
        self.ends_with_newline = chunk.last() == Some(&b'\n');
        self.tail.extend_from_slice(chunk);
        self.trim_tail();

        if self.full_output.is_some() {
            self.append_full_output(chunk).await?;
        } else {
            self.prefix.extend_from_slice(chunk);
            if self.is_truncated() {
                self.create_full_output_file().await?;
            }
        }
        Ok(())
    }

    async fn append_full_output(&mut self, chunk: &[u8]) -> io::Result<()> {
        if self.full_output_capped || chunk.is_empty() {
            return Ok(());
        }
        let cap_label = if self.full_output_limit == MAX_BASH_OUTPUT_BYTES {
            "5GB".to_owned()
        } else {
            format!("{} bytes", self.full_output_limit)
        };
        let cap_note = format!("\n[output truncated: exceeded {cap_label} disk cap]\n");
        let note_bytes = cap_note.as_bytes();
        let note_len = u64::try_from(note_bytes.len())
            .unwrap_or(u64::MAX)
            .min(self.full_output_limit);
        let data_limit = self.full_output_limit.saturating_sub(note_len);
        let remaining = data_limit.saturating_sub(self.full_output_bytes_written);
        let write_len = usize::try_from(remaining)
            .unwrap_or(usize::MAX)
            .min(chunk.len());
        let file = self
            .full_output
            .as_mut()
            .expect("full output file is configured");
        if write_len > 0 {
            file.write_all(&chunk[..write_len]).await?;
            self.full_output_bytes_written = self
                .full_output_bytes_written
                .saturating_add(u64::try_from(write_len).unwrap_or(u64::MAX));
        }
        if write_len < chunk.len() {
            file.write_all(&note_bytes[..usize::try_from(note_len).unwrap_or(note_bytes.len())])
                .await?;
            self.full_output_capped = true;
        }
        Ok(())
    }

    async fn finish(mut self) -> io::Result<BashOutputSnapshot> {
        if let Some(file) = &mut self.full_output {
            file.flush().await?;
        }
        let truncated = self.is_truncated();
        let source = if truncated || self.full_output_path.is_some() {
            &self.tail
        } else {
            &self.prefix
        };
        let decoded = String::from_utf8_lossy(source);
        let mut truncation = truncate_tail(&decoded, self.max_lines, self.max_bytes);
        truncation.total_lines = self.total_lines();
        truncation.total_bytes = self.total_bytes;
        truncation.truncated = truncated;
        Ok(BashOutputSnapshot {
            text: truncation.content,
            truncated,
            truncated_by: truncation.truncated_by,
            total_lines: truncation.total_lines,
            output_lines: truncation.output_lines,
            output_bytes: truncation.output_bytes,
            first_line_partial: truncation.first_line_partial,
            max_bytes: self.max_bytes,
            full_output_path: self.full_output_path,
        })
    }

    fn total_lines(&self) -> usize {
        if self.has_output {
            self.newline_count + usize::from(!self.ends_with_newline)
        } else {
            0
        }
    }

    fn is_truncated(&self) -> bool {
        self.total_bytes > self.max_bytes || self.total_lines() > self.max_lines
    }

    fn trim_tail(&mut self) {
        let rolling_limit = self.max_bytes.saturating_mul(2).max(1);
        if self.tail.len() > rolling_limit.saturating_mul(2) {
            let start = self.tail.len() - rolling_limit;
            self.tail.drain(..start);
        }
    }

    async fn create_full_output_file(&mut self) -> io::Result<()> {
        for _ in 0..10 {
            let path =
                std::env::temp_dir().join(format!("phi-bash-{:016x}.log", fastrand::u64(..)));
            let mut options = OpenOptions::new();
            options.create_new(true).write(true);
            #[cfg(unix)]
            options.mode(0o600);
            match options.open(&path).await {
                Ok(file) => {
                    let prefix = std::mem::take(&mut self.prefix);
                    self.full_output = Some(file);
                    self.full_output_path = Some(path);
                    self.append_full_output(&prefix).await?;
                    return Ok(());
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate a unique bash output file",
        ))
    }
}

struct BashOutputSnapshot {
    text: String,
    truncated: bool,
    truncated_by: Option<TruncatedBy>,
    total_lines: usize,
    output_lines: usize,
    output_bytes: usize,
    first_line_partial: bool,
    max_bytes: usize,
    full_output_path: Option<PathBuf>,
}

fn format_output(snapshot: &BashOutputSnapshot, empty: &str) -> String {
    let mut text = if snapshot.text.is_empty() {
        empty.to_owned()
    } else {
        snapshot.text.clone()
    };
    if snapshot.truncated {
        let path = snapshot.full_output_path.as_ref().map_or_else(
            || "<unavailable>".to_owned(),
            |path| path.display().to_string(),
        );
        let end_line = snapshot.total_lines;
        if snapshot.first_line_partial {
            text = append_status(
                text,
                &format!(
                    "[Showing last {} bytes of line {end_line}. Full output: {path}]",
                    snapshot.output_bytes
                ),
            );
        } else {
            let start_line = end_line
                .saturating_sub(snapshot.output_lines)
                .saturating_add(1);
            let byte_note = if snapshot.truncated_by == Some(TruncatedBy::Bytes) {
                format!(" ({} byte limit)", snapshot.max_bytes)
            } else {
                String::new()
            };
            text = append_status(
                text,
                &format!(
                    "[Showing lines {start_line}-{end_line} of {}{byte_note}. Full output: {path}]",
                    snapshot.total_lines
                ),
            );
        }
    }
    text
}

fn append_status(text: String, status: &str) -> String {
    if text.is_empty() {
        status.to_owned()
    } else {
        format!("{text}\n\n{status}")
    }
}

fn format_exit_status(status: ExitStatus) -> String {
    status.code().map_or_else(
        || "Command terminated by signal".to_owned(),
        |code| format!("Command exited with code {code}"),
    )
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt as _;

    use tempfile::tempdir;

    use super::*;
    use crate::tool::builtins::bash_task::{BashTaskOutputTool, BashTaskStopTool};
    use crate::types::Content;

    fn task_id(output: &ToolOutput) -> String {
        output.metadata.as_ref().unwrap()["task_id"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    fn task_output_text(output: &ToolOutput) -> &str {
        output
            .content
            .split_once("<output>\n")
            .and_then(|(_, output)| output.rsplit_once("\n</output>"))
            .map(|(output, _)| output)
            .unwrap_or_default()
    }

    async fn wait_for_terminal(output_tool: &BashTaskOutputTool, task_id: &str) -> ToolOutput {
        for _ in 0..100 {
            let output = output_tool
                .execute(json!({ "task_id": task_id, "timeout": 2_000 }))
                .await
                .unwrap();
            if output.metadata.as_ref().unwrap()["status"] != "running" {
                return output;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("background task did not finish in time");
    }

    #[tokio::test]
    async fn executes_commands_in_the_configured_directory() {
        let directory = tempdir().unwrap();
        let output = BashTool::new(directory.path())
            .execute(json!({ "command": "printf '%s' \"$PWD\"" }))
            .await
            .unwrap();

        assert_eq!(
            tokio::fs::canonicalize(output.content).await.unwrap(),
            tokio::fs::canonicalize(directory.path()).await.unwrap()
        );
    }

    #[tokio::test]
    async fn returns_nonzero_exit_as_a_tool_error() {
        let directory = tempdir().unwrap();
        let error = BashTool::new(directory.path())
            .execute(json!({ "command": "printf failure; exit 7" }))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("failure"));
        assert!(error.to_string().contains("exited with code 7"));
    }

    #[tokio::test]
    async fn enforces_optional_timeout() {
        let directory = tempdir().unwrap();
        let error = BashTool::new(directory.path())
            .execute(json!({ "command": "sleep 1", "timeout": 0.02 }))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("timed out"));
    }

    #[tokio::test]
    async fn configured_timeout_applies_when_argument_is_omitted() {
        let directory = tempdir().unwrap();
        let mut tool = BashTool::new(directory.path()).without_timeout();
        assert_eq!(tool.configured_timeout(), None);
        tool.set_timeout(Some(Duration::from_millis(20)));

        let error = tool
            .execute(json!({ "command": "sleep 1" }))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("timed out"));
        assert_eq!(tool.configured_timeout(), Some(Duration::from_millis(20)));
    }

    #[test]
    fn has_a_finite_default_timeout() {
        let directory = tempdir().unwrap();
        assert_eq!(
            BashTool::new(directory.path()).configured_timeout(),
            Some(DEFAULT_BASH_TIMEOUT)
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bounds_output_drain_after_shell_exit() {
        let directory = tempdir().unwrap();
        let started = std::time::Instant::now();
        let error = BashTool::new(directory.path())
            .shell("/bin/sh")
            .execute(json!({ "command": "sleep 5 & exit 7", "timeout": 2 }))
            .await
            .unwrap_err();

        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(error.to_string().contains("Output drain exceeded"));
        assert!(error.to_string().contains("exited with code 7"));
        assert!(!error.to_string().contains("unknown"));
    }

    #[tokio::test]
    async fn truncates_from_the_tail_and_persists_full_output() {
        let directory = tempdir().unwrap();
        let output = BashTool::new(directory.path())
            .output_limits(2, 1_024)
            .execute(json!({ "command": "printf 'one\\ntwo\\nthree\\n'" }))
            .await
            .unwrap();

        assert!(output.content.starts_with("two\nthree"));
        let marker = "Full output: ";
        let path = output
            .content
            .split(marker)
            .nth(1)
            .and_then(|value| value.strip_suffix(']'))
            .unwrap();
        assert_eq!(
            tokio::fs::read_to_string(path).await.unwrap(),
            "one\ntwo\nthree\n"
        );
        #[cfg(unix)]
        assert_eq!(
            tokio::fs::metadata(path)
                .await
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        tokio::fs::remove_file(path).await.unwrap();
    }

    #[tokio::test]
    async fn persisted_output_obeys_its_disk_cap() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("capped.output");
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .await
            .unwrap();
        let mut accumulator =
            BashOutputAccumulator::with_full_output_limit(200, 1_024, file, path.clone(), 96);

        accumulator.append(&[b'x'; 200]).await.unwrap();
        accumulator.finish().await.unwrap();

        let output = tokio::fs::read(&path).await.unwrap();
        assert!(output.len() <= 96);
        assert!(String::from_utf8_lossy(&output).contains("output truncated"));
    }

    #[tokio::test]
    async fn background_task_returns_immediately_and_exposes_live_then_final_output() {
        let directory = tempdir().unwrap();
        let tool = BashTool::new(directory.path()).without_timeout();
        let output_tool = BashTaskOutputTool::new(tool.task_registry.clone());
        let stop_tool = BashTaskStopTool::new(tool.task_registry.clone());
        let started = std::time::Instant::now();
        let started_output = tool
            .execute(json!({
                "command": "printf 'started\\n'; sleep 0.3; printf 'finished\\n'",
                "run_in_background": true
            }))
            .await
            .unwrap();
        assert!(started.elapsed() < Duration::from_millis(200));
        let task_id = task_id(&started_output);

        let mut saw_live_output = false;
        for _ in 0..50 {
            let output = output_tool
                .execute(json!({ "task_id": task_id, "block": false }))
                .await
                .unwrap();
            saw_live_output |= output.content.contains("started");
            if output.metadata.as_ref().unwrap()["status"] != "running" {
                assert_eq!(output.metadata.as_ref().unwrap()["status"], "completed");
                assert!(
                    output
                        .content
                        .contains("<retrieval_status>success</retrieval_status>")
                );
                assert!(output.content.contains("<status>completed</status>"));
                assert!(output.content.contains("started"));
                assert!(output.content.contains("finished"));
                assert!(saw_live_output);
                return;
            }
            assert!(
                output
                    .content
                    .contains("<retrieval_status>not_ready</retrieval_status>")
            );
            assert!(output.content.contains("<status>running</status>"));
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        let _ = stop_tool.execute(json!({ "task_id": task_id })).await;
        panic!("background task did not finish in time");
    }

    #[tokio::test]
    async fn background_task_writes_a_readable_file_and_notifies_after_tool_completion() {
        let directory = tempdir().unwrap();
        let tool = BashTool::new(directory.path()).without_timeout();
        let (notifications, mut notification_receiver) =
            tokio::sync::mpsc::unbounded_channel::<Content>();
        let context = ToolExecutionContext::detached("background-call").with_agent_notification(
            Some(Arc::new(move |content| {
                notifications
                    .send(content)
                    .map_err(|_| ToolError::new("notification receiver closed"))
            })),
        );

        let started = tool
            .execute_with_context(
                json!({
                    "command": "sleep 0.05; printf 'notification output\\n'",
                    "description": "Generate notification output",
                    "run_in_background": true
                }),
                context.clone(),
            )
            .await
            .unwrap();
        context.finish();

        assert!(started.content.contains("Do not poll"));
        let output_file = started.metadata.as_ref().unwrap()["output_file"]
            .as_str()
            .unwrap()
            .to_owned();
        let notification =
            tokio::time::timeout(Duration::from_secs(2), notification_receiver.recv())
                .await
                .expect("background task did not notify")
                .expect("notification channel closed");
        let notification = notification.as_text().expect("notification should be text");
        assert!(notification.contains("<task_notification>"));
        assert!(notification.contains("<task_type>local_bash</task_type>"));
        assert!(notification.contains("<status>completed</status>"));
        assert!(notification.contains("Generate notification output"));
        assert!(notification.contains(&output_file));
        assert_eq!(
            tokio::fs::read_to_string(&output_file).await.unwrap(),
            "notification output\n"
        );
        #[cfg(unix)]
        {
            let output_metadata = tokio::fs::metadata(&output_file).await.unwrap();
            assert_eq!(output_metadata.permissions().mode() & 0o777, 0o600);
            let directory_metadata = tokio::fs::metadata(
                std::path::Path::new(&output_file)
                    .parent()
                    .expect("output file has a parent directory"),
            )
            .await
            .unwrap();
            assert_eq!(directory_metadata.permissions().mode() & 0o777, 0o700);
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn dropping_the_last_registry_owner_cancels_background_commands() {
        let directory = tempdir().unwrap();
        let marker = directory.path().join("must-not-exist");
        let tool = BashTool::new(directory.path()).without_timeout();
        tool.execute(json!({
            "command": format!("sleep 0.2; touch '{}'", marker.display()),
            "run_in_background": true
        }))
        .await
        .unwrap();

        drop(tool);
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(!marker.exists());
    }

    #[tokio::test]
    async fn background_task_reports_failed_exit_with_output() {
        let directory = tempdir().unwrap();
        let tool = BashTool::new(directory.path()).without_timeout();
        let output_tool = BashTaskOutputTool::new(tool.task_registry.clone());
        let started_output = tool
            .execute(json!({
                "command": "printf failure; exit 7",
                "run_in_background": true
            }))
            .await
            .unwrap();
        let task_id = task_id(&started_output);

        let output = wait_for_terminal(&output_tool, &task_id).await;
        assert_eq!(output.metadata.as_ref().unwrap()["status"], "failed");
        assert_eq!(output.metadata.as_ref().unwrap()["exit_code"], 7);
        assert!(output.content.contains("failure"));
        assert!(output.content.contains("exited with code 7"));
    }

    #[tokio::test]
    async fn background_task_preserves_timeout_and_reports_it_structurally() {
        let directory = tempdir().unwrap();
        let tool = BashTool::new(directory.path()).without_timeout();
        let output_tool = BashTaskOutputTool::new(tool.task_registry.clone());
        let started_output = tool
            .execute(json!({
                "command": "sleep 30",
                "timeout": 0.02,
                "run_in_background": true
            }))
            .await
            .unwrap();
        let task_id = task_id(&started_output);

        let output = wait_for_terminal(&output_tool, &task_id).await;
        assert_eq!(output.metadata.as_ref().unwrap()["status"], "failed");
        assert_eq!(output.metadata.as_ref().unwrap()["timed_out"], true);
        assert!(output.content.contains("timed out"));
    }

    #[tokio::test]
    async fn stopping_background_task_is_idempotent() {
        let directory = tempdir().unwrap();
        let tool = BashTool::new(directory.path()).without_timeout();
        let output_tool = BashTaskOutputTool::new(tool.task_registry.clone());
        let stop_tool = BashTaskStopTool::new(tool.task_registry.clone());
        let started_output = tool
            .execute(json!({
                "command": "printf ready; sleep 30",
                "run_in_background": true
            }))
            .await
            .unwrap();
        let task_id = task_id(&started_output);
        let mut saw_ready = false;
        for _ in 0..50 {
            let output = output_tool
                .execute(json!({ "task_id": task_id, "block": false }))
                .await
                .unwrap();
            if task_output_text(&output).contains("ready") {
                saw_ready = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(saw_ready, "background task did not expose its live output");

        let stopped = stop_tool
            .execute(json!({ "task_id": task_id }))
            .await
            .unwrap();
        assert_eq!(stopped.metadata.as_ref().unwrap()["status"], "stopped");
        assert!(
            stopped.content.contains("ready"),
            "stopped output was: {}",
            stopped.content
        );
        let stopped_again = stop_tool
            .execute(json!({ "task_id": task_id }))
            .await
            .unwrap();
        assert_eq!(
            stopped_again.metadata.as_ref().unwrap()["status"],
            "stopped"
        );
    }

    #[tokio::test]
    async fn background_task_preserves_full_output_file_for_truncation() {
        let directory = tempdir().unwrap();
        let tool = BashTool::new(directory.path())
            .without_timeout()
            .output_limits(2, 1_024);
        let output_tool = BashTaskOutputTool::new(tool.task_registry.clone());
        let started_output = tool
            .execute(json!({
                "command": "printf 'one\\ntwo\\nthree\\n'",
                "run_in_background": true
            }))
            .await
            .unwrap();
        let task_id = task_id(&started_output);
        let output = wait_for_terminal(&output_tool, &task_id).await;

        assert_eq!(output.metadata.as_ref().unwrap()["exit_code"], 0);
        assert!(task_output_text(&output).starts_with("two\nthree"));
        let path = output.metadata.as_ref().unwrap()["output_file"]
            .as_str()
            .unwrap();
        assert_eq!(
            tokio::fs::read_to_string(path).await.unwrap(),
            "one\ntwo\nthree\n"
        );
        tokio::fs::remove_file(path).await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stopping_background_task_terminates_descendant_process_group() {
        let directory = tempdir().unwrap();
        let tool = BashTool::new(directory.path()).without_timeout();
        let output_tool = BashTaskOutputTool::new(tool.task_registry.clone());
        let stop_tool = BashTaskStopTool::new(tool.task_registry.clone());
        let started_output = tool
            .execute(json!({
                "command": "sleep 30 & printf '%s\\n' $!; wait",
                "run_in_background": true
            }))
            .await
            .unwrap();
        let task_id = task_id(&started_output);

        let mut child_pid = None;
        for _ in 0..100 {
            let output = output_tool
                .execute(json!({ "task_id": task_id, "block": false }))
                .await
                .unwrap();
            if let Ok(pid) = task_output_text(&output)
                .lines()
                .next()
                .unwrap_or_default()
                .parse::<i32>()
            {
                child_pid = Some(pid);
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let child_pid = child_pid.expect("background task did not report its child pid");
        stop_tool
            .execute(json!({ "task_id": task_id }))
            .await
            .unwrap();

        let mut gone = false;
        for _ in 0..50 {
            let result = unsafe { libc::kill(child_pid, 0) };
            if result == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                gone = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            gone,
            "background descendant process {child_pid} survived stop"
        );
    }
}
