//! Bounded desktop helper execution. Helpers never inherit the dashboard's terminal.
use anyhow::{Context, Result, bail, ensure};
use std::{
    io::{self, Read},
    os::{fd::AsRawFd, unix::process::CommandExt},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

struct ChildGuard(Child, bool);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        if !self.1 {
            // SAFETY: this is our unreaped child in a new, private process group.
            unsafe { libc::kill(-(self.0.id() as libc::pid_t), libc::SIGKILL) };
            let _ = self.0.wait();
        }
    }
}

pub(super) fn run(program: &str, args: &[&str], timeout: Duration) -> Result<Vec<u8>> {
    let child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .with_context(|| format!("Cannot start {program}; check that it is installed."))?;
    let mut child = ChildGuard(child, false);
    let mut stdout = child.0.stdout.take().context("Missing helper output")?;
    // SAFETY: stdout owns the fd for this scope; only its nonblocking flag is changed.
    let flags = unsafe { libc::fcntl(stdout.as_raw_fd(), libc::F_GETFL) };
    ensure!(
        flags >= 0
            && unsafe { libc::fcntl(stdout.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) }
                >= 0,
        "Cannot configure helper output"
    );
    let deadline = Instant::now() + timeout;
    let mut output = Vec::new();
    let mut buffer = [0; 8192];
    loop {
        ensure!(Instant::now() < deadline, "{program} timed out; try again.");
        match stdout.read(&mut buffer) {
            Ok(count) if count > 0 => {
                ensure!(
                    output.len() + count <= 8 * 1024 * 1024,
                    "{program} returned too much output"
                );
                output.extend_from_slice(&buffer[..count]);
                continue;
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() != io::ErrorKind::WouldBlock => return Err(error.into()),
            _ => (),
        }
        if let Some(status) = child.0.try_wait()? {
            child.1 = true;
            // Once reaped, drain bytes that arrived between the read and try_wait.
            loop {
                match stdout.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => {
                        ensure!(
                            output.len() + count <= 8 * 1024 * 1024,
                            "{program} returned too much output"
                        );
                        output.extend_from_slice(&buffer[..count]);
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                    Err(error) => return Err(error.into()),
                }
            }
            if !status.success() {
                bail!("{program} could not complete the request");
            }
            return Ok(output);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_output_reports_failures_and_reaps_timed_out_helpers() {
        assert_eq!(
            run("sh", &["-c", "printf focused"], Duration::from_secs(1)).unwrap(),
            b"focused"
        );
        assert!(run("sh", &["-c", "exit 1"], Duration::from_secs(1)).is_err());
        let start = Instant::now();
        assert!(
            run("sh", &["-c", "sleep 10"], Duration::from_millis(50))
                .unwrap_err()
                .to_string()
                .contains("timed out")
        );
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
