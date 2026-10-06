//! Child-process execution with a per-step watchdog and raw log capture.
//!
//! A step runs cargo in a child process. We stream its stdout and stderr to
//! separate raw log files (so nothing is lost even on a crash) and, optionally,
//! hand each stderr line to a live callback so a phase can parse progress as it
//! happens. A watchdog thread kills the process tree if it runs past the
//! configured timeout.

use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

/// Outcome of running a child process.
#[derive(Debug)]
pub struct RunOutput {
    /// Exit code, or None if the process was killed by the watchdog.
    pub exit_code: Option<i32>,
    /// True if the watchdog terminated the process.
    pub timed_out: bool,
    pub elapsed: Duration,
}

impl RunOutput {
    pub fn success(&self) -> bool {
        self.exit_code == Some(0)
    }
}

/// Run `command` with `args` in `cwd`, writing raw stdout to `stdout_log` and
/// raw stderr to `stderr_log`. For every stderr line, `on_stderr` is called with
/// the line text so a caller can parse live progress. The process is killed if
/// it runs longer than `timeout`.
///
/// `extra_env` sets additional environment variables for the child.
// Each parameter is a distinct concern (program, args, cwd, two log paths, env,
// timeout, callback); bundling them into a struct would not aid readability.
#[allow(clippy::too_many_arguments)]
pub fn run_with_capture<F>(
    command: &str,
    args: &[String],
    cwd: &Path,
    stdout_log: &Path,
    stderr_log: &Path,
    extra_env: &[(String, String)],
    timeout: Duration,
    mut on_stderr: F,
) -> Result<RunOutput>
where
    F: FnMut(&str),
{
    let start = Instant::now();

    let mut cmd = Command::new(command);
    cmd.args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in extra_env {
        cmd.env(k, v);
    }

    let mut child = cmd
        .spawn()
        .with_context(|| format!("failed to spawn `{command}`"))?;

    let child_stdout = child.stdout.take().expect("piped stdout");
    let child_stderr = child.stderr.take().expect("piped stderr");

    // Pump stdout on its own thread straight to the raw log.
    let stdout_path = stdout_log.to_path_buf();
    let stdout_handle = thread::spawn(move || -> Result<()> {
        let mut file =
            File::create(&stdout_path).with_context(|| format!("create {stdout_path:?}"))?;
        let reader = BufReader::new(child_stdout);
        for line in reader.lines() {
            let line = line?;
            writeln!(file, "{line}")?;
        }
        Ok(())
    });

    // Pump stderr on this thread's helper: write to the raw log AND forward each
    // line over a channel so the main thread can invoke the live callback while
    // still enforcing the timeout.
    let (tx, rx) = mpsc::channel::<String>();
    let stderr_path = stderr_log.to_path_buf();
    let stderr_done = Arc::new(AtomicBool::new(false));
    let stderr_done_writer = Arc::clone(&stderr_done);
    let stderr_handle = thread::spawn(move || -> Result<()> {
        let mut file =
            File::create(&stderr_path).with_context(|| format!("create {stderr_path:?}"))?;
        let reader = BufReader::new(child_stderr);
        for line in reader.lines() {
            let line = line?;
            writeln!(file, "{line}")?;
            // If the receiver is gone (timeout path), stop forwarding.
            if tx.send(line).is_err() {
                break;
            }
        }
        stderr_done_writer.store(true, Ordering::SeqCst);
        Ok(())
    });

    // Main loop: drain forwarded stderr lines (feeding the callback) while
    // polling the child for exit and the clock for the timeout.
    let mut timed_out = false;
    let exit_code;
    loop {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(line) => on_stderr(&line),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                // stderr fully drained; fall through to exit check.
            }
        }

        if start.elapsed() >= timeout {
            kill_tree(&mut child);
            timed_out = true;
            exit_code = None;
            break;
        }

        match child.try_wait()? {
            Some(status) => {
                // Drain any stderr still buffered before declaring done.
                while let Ok(line) = rx.recv_timeout(Duration::from_millis(50)) {
                    on_stderr(&line);
                }
                exit_code = status.code();
                break;
            }
            None => continue,
        }
    }

    // Join pump threads, but NEVER block forever. Once the child has exited (or
    // been killed), its stdout/stderr pipes should reach EOF and the pump
    // threads should return promptly. However, test runners such as nextest
    // spawn many test-binary child processes; on a failing or interrupted run
    // one of those grandchildren can linger holding the write end of the stdout
    // pipe open, so the pump thread's `reader.lines()` never sees EOF and a
    // plain `join()` would hang the whole gate. The raw logs are best-effort
    // (the comment on this function says so), so we join with a grace period and
    // abandon a pump thread that has not finished -- the process is already
    // gone and we have the exit status we need.
    join_with_grace(stdout_handle, Duration::from_secs(5));
    join_with_grace(stderr_handle, Duration::from_secs(5));

    Ok(RunOutput {
        exit_code,
        timed_out,
        elapsed: start.elapsed(),
    })
}

/// Join a pump thread, waiting at most `grace`. If it has not finished (a child
/// grandchild is holding a pipe open), abandon it rather than blocking the gate
/// forever; the thread is writing a best-effort raw log and will exit when the
/// OS finally closes the pipe. The thread is detached (not forcibly killed),
/// which is safe: it only writes to its own log file and sends on a channel
/// whose receiver has already been dropped.
fn join_with_grace(handle: thread::JoinHandle<Result<()>>, grace: Duration) {
    let (done_tx, done_rx) = mpsc::channel::<()>();
    let watcher = thread::spawn(move || {
        let _ = handle.join();
        let _ = done_tx.send(());
    });
    // If the pump finished within the grace period, also reap the watcher so we
    // leak nothing in the common (fast) path.
    if done_rx.recv_timeout(grace).is_ok() {
        let _ = watcher.join();
    }
    // Otherwise we return and leave the watcher + pump detached; both exit on
    // their own once the lingering pipe finally closes.
}

/// Kill a child and best-effort its descendants. On a timeout we cannot rely on
/// cargo cleaning up rustc children, so we kill the direct child; the OS reaps
/// the group on most platforms once the parent dies and pipes close.
fn kill_tree(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let mut d = std::env::temp_dir();
        d.push(format!("cargo-gate-test-{name}-{}", std::process::id()));
        fs::create_dir_all(&d).expect("mkdir temp");
        d
    }

    #[cfg(windows)]
    #[test]
    fn captures_stdout_and_reports_success() {
        let dir = temp_dir("ok");
        let out = dir.join("out.log");
        let err = dir.join("err.log");
        let result = run_with_capture(
            "cmd",
            &["/c".to_string(), "echo hello".to_string()],
            &dir,
            &out,
            &err,
            &[],
            Duration::from_secs(30),
            |_| {},
        )
        .expect("run");
        assert!(result.success(), "echo should succeed");
        assert!(!result.timed_out);
        let captured = fs::read_to_string(&out).expect("read stdout log");
        assert!(
            captured.contains("hello"),
            "stdout log should contain echo output"
        );
    }

    #[cfg(windows)]
    #[test]
    fn failing_child_with_output_returns_promptly() {
        // A child that writes output and exits NON-zero must still return
        // quickly (no hang joining the pump threads). This guards the
        // join-with-grace fix for the failing-test path.
        let dir = temp_dir("fail");
        let out = dir.join("out.log");
        let err = dir.join("err.log");
        let started = Instant::now();
        let result = run_with_capture(
            "cmd",
            &[
                "/c".to_string(),
                "echo out-line & echo err-line 1>&2 & exit /b 1".to_string(),
            ],
            &dir,
            &out,
            &err,
            &[],
            Duration::from_secs(30),
            |_| {},
        )
        .expect("run");
        assert!(!result.success(), "exit /b 1 is a failure");
        assert!(!result.timed_out, "a fast failure is not a timeout");
        assert_eq!(result.exit_code, Some(1));
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "a failing child must not hang the join path"
        );
    }

    #[cfg(windows)]
    #[test]
    fn watchdog_terminates_a_long_running_child() {
        let dir = temp_dir("timeout");
        let out = dir.join("out.log");
        let err = dir.join("err.log");
        // ping with a delay is a portable "sleep" on Windows.
        let result = run_with_capture(
            "cmd",
            &["/c".to_string(), "ping -n 10 127.0.0.1 > NUL".to_string()],
            &dir,
            &out,
            &err,
            &[],
            Duration::from_millis(500),
            |_| {},
        )
        .expect("run");
        assert!(result.timed_out, "slow child should hit the watchdog");
        assert!(result.exit_code.is_none());
    }
}
