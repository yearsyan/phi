use std::{
    io,
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
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
    common::{invalid_arguments, normalize_cwd},
    truncate::{DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES, TruncatedBy, truncate_tail},
};
use crate::{
    error::ToolError,
    tool::{Tool, ToolOutput},
    types::ToolDefinition,
};

const MAX_TIMEOUT_SECONDS: f64 = 2_147_483_647_f64 / 1_000_f64;

#[derive(Clone, Debug)]
pub struct BashTool {
    cwd: PathBuf,
    shell: PathBuf,
    max_lines: usize,
    max_bytes: usize,
}

impl BashTool {
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self {
            cwd: normalize_cwd(cwd),
            shell: default_shell(),
            max_lines: DEFAULT_MAX_LINES,
            max_bytes: DEFAULT_MAX_BYTES,
        }
    }

    pub fn shell(mut self, shell: impl Into<PathBuf>) -> Self {
        self.shell = shell.into();
        self
    }

    pub fn output_limits(mut self, max_lines: usize, max_bytes: usize) -> Self {
        self.max_lines = max_lines.max(1);
        self.max_bytes = max_bytes.max(1);
        self
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BashArguments {
    command: String,
    timeout: Option<f64>,
}

#[async_trait]
impl Tool for BashTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "bash",
            format!(
                "Execute a shell command in the configured working directory. Returns combined stdout and stderr, truncated to the last {} lines or {} bytes. Truncated full output is saved to a temporary file. An optional timeout is measured in seconds.",
                self.max_lines, self.max_bytes
            ),
            json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "Shell command to execute"
                    },
                    "timeout": {
                        "type": "number",
                        "exclusiveMinimum": 0,
                        "description": "Optional timeout in seconds; no timeout is applied when omitted"
                    }
                },
                "required": ["command"],
                "additionalProperties": false
            }),
        )
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, ToolError> {
        let arguments: BashArguments =
            serde_json::from_value(arguments).map_err(|error| invalid_arguments("bash", error))?;
        let timeout = resolve_timeout(arguments.timeout)?;
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
            let mut accumulator = BashOutputAccumulator::new(max_lines, max_bytes);
            collect_output(receiver, &mut accumulator).await?;
            accumulator.finish().await
        });

        let wait_outcome = wait_for_child(&mut child, timeout).await;
        let stdout_result = join_reader(stdout_task, "stdout").await;
        let stderr_result = join_reader(stderr_task, "stderr").await;
        let snapshot = collector
            .await
            .map_err(|error| ToolError::new(format!("bash output collector failed: {error}")))?
            .map_err(|error| ToolError::new(format!("could not collect bash output: {error}")))?;
        stdout_result?;
        stderr_result?;

        match wait_outcome {
            WaitOutcome::Exited(status) => {
                let status = status.map_err(|error| {
                    ToolError::new(format!("failed waiting for command to exit: {error}"))
                })?;
                if status.success() {
                    Ok(ToolOutput::success(format_output(&snapshot, "(no output)")))
                } else {
                    let output = format_output(&snapshot, "");
                    Err(ToolError::new(append_status(
                        output,
                        &format_exit_status(status),
                    )))
                }
            }
            WaitOutcome::TimedOut(duration) => {
                let output = format_output(&snapshot, "");
                Err(ToolError::new(append_status(
                    output,
                    &format!("Command timed out after {} seconds", duration.as_secs_f64()),
                )))
            }
        }
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

fn resolve_timeout(timeout: Option<f64>) -> Result<Option<Duration>, ToolError> {
    let Some(timeout) = timeout else {
        return Ok(None);
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

async fn join_reader(task: JoinHandle<io::Result<()>>, stream: &str) -> Result<(), ToolError> {
    task.await
        .map_err(|error| ToolError::new(format!("bash {stream} reader failed: {error}")))?
        .map_err(|error| ToolError::new(format!("could not read bash {stream}: {error}")))
}

async fn collect_output(
    mut receiver: mpsc::Receiver<Vec<u8>>,
    accumulator: &mut BashOutputAccumulator,
) -> io::Result<()> {
    while let Some(chunk) = receiver.recv().await {
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

        if let Some(file) = &mut self.full_output {
            file.write_all(chunk).await?;
        } else {
            self.prefix.extend_from_slice(chunk);
            if self.is_truncated() {
                self.create_full_output_file().await?;
            }
        }
        Ok(())
    }

    async fn finish(mut self) -> io::Result<BashOutputSnapshot> {
        if let Some(file) = &mut self.full_output {
            file.flush().await?;
        }
        let truncated = self.is_truncated();
        let source = if truncated { &self.tail } else { &self.prefix };
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
            match OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&path)
                .await
            {
                Ok(mut file) => {
                    file.write_all(&self.prefix).await?;
                    self.prefix.clear();
                    self.full_output = Some(file);
                    self.full_output_path = Some(path);
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
    use tempfile::tempdir;

    use super::*;

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
        tokio::fs::remove_file(path).await.unwrap();
    }
}
