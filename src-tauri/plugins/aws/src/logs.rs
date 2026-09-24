// CloudWatch logs live tail — `aws logs tail <group> --follow` streams new
// events to stdout indefinitely. The child is killed when the host drops the
// stream.

use std::process::Stdio;

use serde_json::Value;
use sikemux_plugin_api::{PluginResult, StreamSink};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::common::aws_bin;
use crate::error::AwsError;

pub(crate) async fn tail(
    profile: String,
    log_group: String,
    log_stream: Option<String>,
    since: Option<String>,
    sink: StreamSink,
) -> PluginResult<()> {
    let bin = aws_bin();
    let mut cmd = Command::new(&bin);
    cmd.env("AWS_PROFILE", &profile)
        .env("AWS_PAGER", "")
        .env("NO_COLOR", "1")
        // The AWS CLI's embedded Python block-buffers stdout when it is a
        // pipe, so a quiet log stream would never reach us without this.
        .env("PYTHONUNBUFFERED", "1")
        .arg("logs")
        .arg("tail")
        .arg(&log_group)
        .arg("--follow")
        .arg("--format")
        .arg("short");
    if let Some(stream) = &log_stream {
        cmd.arg("--log-stream-names").arg(stream);
    }
    cmd.arg("--since").arg(since.as_deref().unwrap_or("5m"));
    cmd.stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = cmd.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            AwsError::CliMissing(bin.clone())
        } else {
            AwsError::Io(e)
        }
    })?;
    let stdout = child
        .stdout
        .take()
        .ok_or(AwsError::Aws("no stdout".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or(AwsError::Aws("no stderr".into()))?;

    let mut out_lines = BufReader::new(stdout).lines();
    let mut err_lines = BufReader::new(stderr).lines();
    let mut stderr_open = true;
    loop {
        tokio::select! {
            line = out_lines.next_line() => match line {
                Ok(Some(line)) => sink.send(Value::String(line))?,
                _ => break,
            },
            line = err_lines.next_line(), if stderr_open => match line {
                Ok(Some(line)) => sink.send(Value::String(format!("[err] {line}")))?,
                _ => stderr_open = false,
            },
        }
    }
    while stderr_open {
        match err_lines.next_line().await {
            Ok(Some(line)) => sink.send(Value::String(format!("[err] {line}")))?,
            _ => stderr_open = false,
        }
    }
    let _ = child.wait().await;
    Ok(())
}
