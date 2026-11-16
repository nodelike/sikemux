use std::io;
use std::process::{ChildStderr, ChildStdin, ChildStdout, ExitStatus, Output};

pub(super) use std::process::{Child, Command, Stdio};
pub(super) use tokio::process::{Child as AsyncChild, Command as AsyncCommand};

pub(super) trait Std {
    fn std(&self) -> &Command;
    fn std_mut(&mut self) -> &mut Command;
}

impl Std for Command {
    fn std(&self) -> &Command {
        self
    }

    fn std_mut(&mut self) -> &mut Command {
        self
    }
}

impl Std for AsyncCommand {
    fn std(&self) -> &Command {
        self.as_std()
    }

    fn std_mut(&mut self) -> &mut Command {
        self.as_std_mut()
    }
}

pub(super) fn command(inner: &mut impl Std) -> &mut Command {
    inner.std_mut()
}

pub(super) fn command_ref(inner: &impl Std) -> &Command {
    inner.std()
}

type Started = (
    Child,
    Option<ChildStdin>,
    Option<ChildStdout>,
    Option<ChildStderr>,
);

#[inline]
pub(super) fn spawn(command: &mut Command) -> io::Result<Started> {
    let mut child = command.spawn()?;
    let (stdin, stdout, stderr) = (child.stdin.take(), child.stdout.take(), child.stderr.take());
    Ok((child, stdin, stdout, stderr))
}

#[inline]
pub(super) fn output(command: &mut Command) -> io::Result<Output> {
    command.output()
}

#[inline]
pub(super) fn status(command: &mut Command) -> io::Result<ExitStatus> {
    command.status()
}

#[inline]
pub(super) fn wait_with_output(
    mut child: Child,
    stdout: Option<ChildStdout>,
    stderr: Option<ChildStderr>,
) -> io::Result<Output> {
    child.stdout = stdout;
    child.stderr = stderr;
    child.wait_with_output()
}

#[inline]
pub(super) fn into_async(command: Command) -> AsyncCommand {
    AsyncCommand::from(command)
}

type StartedAsync = (
    AsyncChild,
    Option<tokio::process::ChildStdin>,
    Option<tokio::process::ChildStdout>,
    Option<tokio::process::ChildStderr>,
);

#[inline]
pub(super) fn spawn_async(command: &mut AsyncCommand) -> io::Result<StartedAsync> {
    let mut child = command.spawn()?;
    let (stdin, stdout, stderr) = (child.stdin.take(), child.stdout.take(), child.stderr.take());
    Ok((child, stdin, stdout, stderr))
}

#[inline]
pub(super) async fn output_async(command: &mut AsyncCommand) -> io::Result<Output> {
    command.output().await
}

#[inline]
pub(super) async fn status_async(command: &mut AsyncCommand) -> io::Result<ExitStatus> {
    command.status().await
}

#[inline]
pub(super) async fn wait_with_output_async(
    mut child: AsyncChild,
    stdout: Option<tokio::process::ChildStdout>,
    stderr: Option<tokio::process::ChildStderr>,
) -> io::Result<Output> {
    child.stdout = stdout;
    child.stderr = stderr;
    child.wait_with_output().await
}
