#![cfg(target_os = "linux")]

use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::Path,
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use tempfile::tempdir;

const KAI: &str = env!("CARGO_BIN_EXE_kai");
const TIMEOUT: Duration = Duration::from_secs(15);

// All fixture children have their own process group, including shell sleep children.
struct Running(Child);
impl Running {
    fn stop(&mut self) {
        if self.0.try_wait().unwrap().is_none() {
            // SAFETY: this group belongs to the unreaped test child created with process_group(0).
            unsafe {
                libc::kill(-(self.0.id() as i32), libc::SIGKILL);
            }
            self.0.wait().unwrap();
        }
    }
}
impl Drop for Running {
    fn drop(&mut self) {
        self.stop();
    }
}

fn lifecycle(path: &Path, kind: &str, turn: &str) {
    let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
    writeln!(
        file,
        "{}",
        json!({"timestamp":"2026-09-30T12:00:00Z", "type":"event_msg",
        "payload":{"type":kind, "turn_id":turn}})
    )
    .unwrap();
}

#[test]
fn watches_existing_windows_through_completion_new_work_and_exit() {
    let root = tempdir().unwrap();
    let log = root.path().join("rollout-fixture.jsonl");
    fs::write(
        &log,
        format!(
            "{}\n",
            json!({"type":"session_meta", "payload":{
                "id":"fixture-thread", "source":"cli", "cwd":root.path()
            }})
        ),
    )
    .unwrap();
    lifecycle(&log, "task_started", "first");
    let codex = root.path().join("codex");
    fs::write(
        &codex,
        r#"#!/bin/sh
if [ "$1" = --version ]; then
    echo 'codex-cli 0.158.0'
    exit 0
fi
exec 3< "$KAI_STATUS_FIXTURE_LOG"
: > "$KAI_STATUS_FIXTURE_READY"
while :; do /bin/sleep 1; done
"#,
    )
    .unwrap();
    fs::set_permissions(&codex, fs::Permissions::from_mode(0o700)).unwrap();
    let ready = root.path().join("ready");
    let mut launcher = Running(
        Command::new(KAI)
            .env("PATH", root.path())
            .env("KAI_STATUS_FIXTURE_LOG", &log)
            .env("KAI_STATUS_FIXTURE_READY", &ready)
            .current_dir(root.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + TIMEOUT;
    while !ready.exists() {
        assert!(Instant::now() < deadline, "fixture Codex did not start");
        assert!(
            launcher.0.try_wait().unwrap().is_none(),
            "fixture Kai exited early"
        );
        thread::sleep(Duration::from_millis(20));
    }
    let pid = launcher.0.id();
    let mut watcher = Running(
        Command::new(KAI)
            .args(["status", "--watch", "--json", "--interval", "1"])
            .env("PATH", "")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .unwrap(),
    );
    let reader = BufReader::new(watcher.0.stdout.take().unwrap());
    let (sender, receiver) = mpsc::channel();
    let reading = thread::spawn(move || {
        for line in reader.lines() {
            let Ok(line) = line else {
                break;
            };
            let snapshot: Value = serde_json::from_str(&line).unwrap();
            if sender.send(snapshot).is_err() {
                break;
            }
        }
    });
    let wait_for = |state: &str| -> Value {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let snapshot = receiver
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap();
            assert_eq!(snapshot["version"], 1);
            assert!(
                !snapshot["windows"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|row| row["pid"] == watcher.0.id())
            );
            if let Some(row) = snapshot["windows"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["pid"] == pid && row["state"] == state)
            {
                return row.clone();
            }
        }
    };
    assert_eq!(wait_for("working")["thread_id"], "fixture-thread");
    lifecycle(&log, "task_complete", "first");
    let done = wait_for("ready");
    assert!(done["last_finished_at"].is_i64());
    lifecycle(&log, "task_started", "second");
    assert_eq!(
        wait_for("working")["last_finished_at"],
        done["last_finished_at"]
    );
    launcher.stop();
    assert!(wait_for("exited")["exited_at"].is_u64());
    watcher.stop();
    reading.join().unwrap();
}

#[test]
fn status_runs_without_codex_and_rejects_invalid_options() {
    let output = Command::new(KAI)
        .args(["status", "--json"])
        .env("PATH", "")
        .output()
        .unwrap();
    assert!(output.status.success());
    let snapshot: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(snapshot["version"], 1);
    for args in [
        vec!["status", "--interval", "0"],
        vec!["status", "--fast"],
        vec!["status", "--watch"],
    ] {
        let output = Command::new(KAI)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(!output.status.success());
    }
}

#[test]
fn json_watch_exits_cleanly_when_its_pipe_closes() {
    let mut watcher = Running(
        Command::new(KAI)
            .args(["status", "--watch", "--json", "--interval", "1"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .unwrap(),
    );
    // Closing the read end immediately must make even the first JSON write a clean exit.
    drop(watcher.0.stdout.take());
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Some(status) = watcher.0.try_wait().unwrap() {
            assert!(status.success());
            let mut stderr = String::new();
            watcher
                .0
                .stderr
                .take()
                .unwrap()
                .read_to_string(&mut stderr)
                .unwrap();
            assert!(stderr.is_empty());
            break;
        }
        assert!(
            Instant::now() < deadline,
            "watcher did not handle the closed pipe"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn terminal_watch_restores_the_terminal_after_q_and_control_c() {
    for quit in [b'q', 3] {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 30,
                cols: 120,
                ..PtySize::default()
            })
            .unwrap();
        let before = pair.master.get_termios().unwrap();
        let mut command = CommandBuilder::new(KAI);
        command.args(["status", "--watch", "--interval", "1"]);
        let mut child = pair.slave.spawn_command(command).unwrap();
        let mut writer = pair.master.take_writer().unwrap();
        let mut reader = pair.master.try_clone_reader().unwrap();
        let (sender, receiver) = mpsc::channel();
        let reading = thread::spawn(move || {
            let mut bytes = [0; 4096];
            while let Ok(count) = reader.read(&mut bytes) {
                if count == 0 || sender.send(bytes[..count].to_vec()).is_err() {
                    break;
                }
            }
        });
        let deadline = Instant::now() + TIMEOUT;
        let mut output = Vec::new();
        while !String::from_utf8_lossy(&output).contains("Kai windows:") {
            match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(bytes) => output.extend(bytes),
                Err(error) => {
                    child.kill().unwrap();
                    child.wait().unwrap();
                    panic!("dashboard did not render: {error}");
                }
            }
        }
        writer.write_all(&[quit]).unwrap();
        writer.flush().unwrap();
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("dashboard did not quit");
            }
            thread::sleep(Duration::from_millis(20));
        }
        let after = pair.master.get_termios().unwrap();
        assert_eq!(before.local_flags, after.local_flags);
        assert_eq!(before.input_flags, after.input_flags);
        assert_eq!(before.output_flags, after.output_flags);
        drop(pair.slave);
        reading.join().unwrap();
        output.extend(receiver.try_iter().flatten());
        assert!(
            output
                .windows(b"\x1b[?1049l".len())
                .any(|part| part == b"\x1b[?1049l")
        );
        assert!(
            output
                .windows(b"\x1b[?25h".len())
                .any(|part| part == b"\x1b[?25h")
        );
    }
}
