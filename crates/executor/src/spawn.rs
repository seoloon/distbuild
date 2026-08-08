use std::path::Path;
use std::process::Stdio;

use protocol::LogStream;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

#[derive(Debug, Error)]
pub enum ExecutorError {
    #[error("failed to spawn `{program}`: {source}")]
    Spawn {
        program: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to wait for `{program}`: {source}")]
    Wait {
        program: String,
        #[source]
        source: std::io::Error,
    },
}

/// One line of captured output, tagged with the stream it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    pub stream: LogStream,
    pub data: String,
}

/// The outcome of a single [`run_step`] invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StepResult {
    pub success: bool,
    pub exit_code: Option<i32>,
}

/// Spawns `program args` in `working_dir`, forwarding each stdout/stderr
/// line to `tx` as it is produced (line-buffered, per PROMPT.md's
/// executor requirements), and returns once the process exits.
pub async fn run_step(
    program: &str,
    args: &[String],
    working_dir: &Path,
    tx: &mpsc::UnboundedSender<LogLine>,
) -> Result<StepResult, ExecutorError> {
    let mut child = Command::new(program)
        .args(args)
        .current_dir(working_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| ExecutorError::Spawn {
            program: program.to_string(),
            source,
        })?;

    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");

    let stdout_tx = tx.clone();
    let stdout_task = tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let _ = stdout_tx.send(LogLine {
                stream: LogStream::Stdout,
                data: line,
            });
        }
    });

    let stderr_tx = tx.clone();
    let stderr_task = tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let _ = stderr_tx.send(LogLine {
                stream: LogStream::Stderr,
                data: line,
            });
        }
    });

    let status = child.wait().await.map_err(|source| ExecutorError::Wait {
        program: program.to_string(),
        source,
    })?;

    let _ = stdout_task.await;
    let _ = stderr_task.await;

    Ok(StepResult {
        success: status.success(),
        exit_code: status.code(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    fn echo_command() -> (&'static str, Vec<String>) {
        (
            "cmd",
            vec!["/C".to_string(), "echo hello-from-executor".to_string()],
        )
    }

    #[cfg(not(windows))]
    fn echo_command() -> (&'static str, Vec<String>) {
        (
            "sh",
            vec!["-c".to_string(), "echo hello-from-executor".to_string()],
        )
    }

    #[tokio::test]
    async fn run_step_captures_stdout_lines_and_exit_code() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (program, args) = echo_command();
        let (tx, mut rx) = mpsc::unbounded_channel();

        let result = run_step(program, &args, dir.path(), &tx)
            .await
            .expect("run_step should succeed");

        drop(tx);
        let mut lines = Vec::new();
        while let Some(line) = rx.recv().await {
            lines.push(line);
        }

        assert!(result.success);
        assert_eq!(result.exit_code, Some(0));
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].stream, LogStream::Stdout);
        assert_eq!(lines[0].data, "hello-from-executor");
    }

    #[cfg(windows)]
    fn failing_command() -> (&'static str, Vec<String>) {
        ("cmd", vec!["/C".to_string(), "exit 3".to_string()])
    }

    #[cfg(not(windows))]
    fn failing_command() -> (&'static str, Vec<String>) {
        ("sh", vec!["-c".to_string(), "exit 3".to_string()])
    }

    #[tokio::test]
    async fn run_step_reports_nonzero_exit_code() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (program, args) = failing_command();
        let (tx, _rx) = mpsc::unbounded_channel();

        let result = run_step(program, &args, dir.path(), &tx)
            .await
            .expect("run_step should succeed even when the child fails");

        assert!(!result.success);
        assert_eq!(result.exit_code, Some(3));
    }

    #[tokio::test]
    async fn run_step_returns_spawn_error_for_missing_program() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (tx, _rx) = mpsc::unbounded_channel();

        let result = run_step("distbuild-nonexistent-program-xyz", &[], dir.path(), &tx).await;

        assert!(matches!(result, Err(ExecutorError::Spawn { .. })));
    }
}
