//! Running a command with a deadline. `std::process` has none, and a child that fills a pipe
//! while nobody reads it never exits, so stdout and stderr are drained on their own threads.

use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};
use wait_timeout::ChildExt;

/// What a finished command printed, and how it exited (`None`: killed by a signal).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Output {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug)]
pub enum RunError {
    /// It couldn't be started (not installed, no such folder).
    Spawn(std::io::Error),
    /// It ran past its deadline and was killed.
    Timeout(Duration),
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunError::Spawn(e) => write!(f, "{e}"),
            RunError::Timeout(d) => write!(f, "no answer after {} seconds", d.as_secs()),
        }
    }
}

impl std::error::Error for RunError {}

/// A started command whose output is being collected.
pub struct Running {
    child: Child,
    stdout: Receiver<Vec<u8>>,
    stderr: Option<Receiver<Vec<u8>>>,
}

/// Start `argv` in `cwd` (or here). stdin is the board's own when `inherit_stdin`, as for the
/// `claude --bg` family, else empty.
pub fn spawn(argv: &[String], cwd: Option<&Path>, inherit_stdin: bool, capture_stderr: bool) -> Result<Running, RunError> {
    let (program, args) = argv.split_first().ok_or_else(|| {
        RunError::Spawn(std::io::Error::new(std::io::ErrorKind::InvalidInput, "empty command"))
    })?;
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(if inherit_stdin { Stdio::inherit() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(if capture_stderr { Stdio::piped() } else { Stdio::null() });
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let mut child = cmd.spawn().map_err(RunError::Spawn)?;
    let stdout = drain(child.stdout.take());
    let stderr = child.stderr.take().map(|e| drain(Some(e)));
    Ok(Running { child, stdout, stderr })
}

fn drain(pipe: Option<impl Read + Send + 'static>) -> Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    if let Some(mut pipe) = pipe {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = pipe.read_to_end(&mut buf);
            let _ = tx.send(buf);
        });
    }
    rx
}

impl Running {
    /// Wait up to `timeout`; past it the command is killed.
    pub fn wait(mut self, timeout: Duration) -> Result<Output, RunError> {
        let deadline = Instant::now() + timeout;
        let status = match self.child.wait_timeout(timeout) {
            Ok(Some(status)) => status,
            Ok(None) | Err(_) => {
                let _ = self.child.kill();
                let _ = self.child.wait();
                return Err(RunError::Timeout(timeout));
            }
        };
        // A grandchild that kept the pipe open mustn't hold us past the deadline (plus a moment).
        let rest = deadline.saturating_duration_since(Instant::now()) + Duration::from_millis(500);
        let text = |rx: &Receiver<Vec<u8>>| {
            rx.recv_timeout(rest).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default()
        };
        Ok(Output {
            code: status.code(),
            stdout: text(&self.stdout),
            stderr: self.stderr.as_ref().map(text).unwrap_or_default(),
        })
    }
}

/// Run `argv` to completion within `timeout`.
pub fn run(argv: &[String], cwd: Option<&Path>, timeout: Duration) -> Result<Output, RunError> {
    spawn(argv, cwd, true, true)?.wait(timeout)
}
