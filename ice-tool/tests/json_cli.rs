#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::{Value, json};
use tempfile::TempDir;

struct Fixture {
    root: TempDir,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("bin")).unwrap();
        fs::create_dir(root.path().join("config")).unwrap();
        fs::create_dir(root.path().join("runtime")).unwrap();
        Self { root }
    }

    fn provider(&self, rows: Value, failure: bool) {
        fs::write(self.root.path().join("response.json"), rows.to_string()).unwrap();
        let script = if failure {
            "#!/bin/sh\nprintf '%s\\n' 'provider unavailable' >&2\nexit 7\n".to_owned()
        } else {
            "#!/bin/sh\nprintf '%s\\n' 'provider diagnostic' >&2\n/bin/cat \"$ICE_TEST_RESPONSE\"\n"
                .to_owned()
        };
        self.script("gcloud", &script);
    }

    fn script(&self, name: &str, script: &str) {
        let path = self.root.path().join("bin").join(name);
        fs::write(&path, script).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ice"));
        command
            .env("PATH", self.root.path().join("bin"))
            .env("XDG_CONFIG_HOME", self.root.path().join("config"))
            .env("XDG_RUNTIME_DIR", self.root.path().join("runtime"))
            .env("ICE_TEST_RESPONSE", self.root.path().join("response.json"))
            .env("ICE_TEST_STATE", self.root.path().join("state"))
            .env("ICE_TEST_ACTIONS", self.root.path().join("actions"))
            .env_remove("AWS_ACCESS_KEY_ID")
            .env_remove("AWS_SECRET_ACCESS_KEY")
            .env_remove("AWS_SESSION_TOKEN");
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }

    fn unpack(&self, logs: &[u8], exit_code: Option<i32>) -> PathBuf {
        let dir = self
            .root
            .path()
            .join("config/ice/provider/local/unpack/ice-example");
        fs::create_dir_all(dir.join("rootfs")).unwrap();
        fs::write(
            dir.join("instance.toml"),
            concat!(
                "id = 'ice-example'\nname = 'ice-example'\nsource = 'example:tag'\n",
                "created_at = '2026-01-01T00:00:00Z'\n",
            ),
        )
        .unwrap();
        fs::write(dir.join("stdio.log"), logs).unwrap();
        if let Some(code) = exit_code {
            fs::write(dir.join("exit-code"), code.to_string()).unwrap();
        }
        dir
    }

    fn runtime(&self) {
        self.script("docker", r#"#!/bin/sh
case "$1" in
  version) exit 0 ;;
  ps) printf 'test-id\n' ;;
  inspect)
    state=running
    if [ -f "$ICE_TEST_STATE" ]; then state=$(/bin/cat "$ICE_TEST_STATE"); fi
    printf '[{"Id":"test-id","Name":"/ice-example","State":{"Status":"%s"},"Config":{"Image":"example:tag","Labels":{"ice-managed":"true","ice-cloud":"local","ice-workload-kind":"container","ice-workload-container":"us-central1-docker.pkg.dev/test/repo/image:tag","ice-workload-registry":"gar"}}}]\n' "$state"
    ;;
  stop) printf stopped > "$ICE_TEST_STATE"; printf 'provider stop output\n' ;;
  start) printf running > "$ICE_TEST_STATE"; printf 'provider start output\n' ;;
  logs) printf 'stdout 🦀\n'; printf 'stderr 終' >&2 ;;
  login) /bin/cat >/dev/null; printf 'registry login output\n' ;;
  *) printf 'provider operation output\n' ;;
esac
printf '%s\n' "$1" >> "$ICE_TEST_ACTIONS"
"#);
    }
}

fn parsed(output: Output) -> Value {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "stdout is not a single JSON document: {error}: {}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

fn failed(output: Output) -> Value {
    assert!(!output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("invalid error JSON: {error}: {:?}", output));
    assert!(value.get("result").is_none());
    assert!(value["error"]["message"].is_string());
    value
}

fn records(bytes: &[u8]) -> Vec<Value> {
    serde_json::Deserializer::from_slice(bytes)
        .into_iter::<Value>()
        .map(Result::unwrap)
        .collect()
}

#[test]
fn list_projects_provider_records_without_secrets_or_display_formatting() {
    let fixture = Fixture::new();
    fixture.provider(
        json!([{
            "name": "ice-example", "status": "RUNNING", "zone": "zones/us-central1-a",
            "machineType": "machineTypes/g2-standard-4",
            "metadata": {"items": [{"key": "startup-script", "value": "secret-startup-token"}]},
            "credentials": "secret-provider-token"
        }]),
        false,
    );
    let value = parsed(fixture.run(&["list", "--cloud", "gcp", "--json"]));
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["command"], "list");
    assert_eq!(value["cloud"], "gcp");
    assert_eq!(value["result"]["instances"][0]["id"], "ice-example");
    assert_eq!(value["result"]["instances"][0]["state"], "RUNNING");
    assert_eq!(value["result"]["instances"][0]["zone"], "us-central1-a");
    assert!(value["result"]["instances"][0]["created_at"].is_null());
    assert!(!value.to_string().contains("secret-"));
    assert!(!value.to_string().contains("color"));
}

#[test]
fn empty_list_is_a_success_but_provider_failure_has_no_success_output() {
    let fixture = Fixture::new();
    fixture.provider(json!([]), false);
    let empty = parsed(fixture.run(&["list", "--cloud", "gcp", "--json"]));
    assert_eq!(empty["result"]["instances"], json!([]));
    fixture.provider(json!([]), true);
    let failure = fixture.run(&["list", "--cloud", "gcp", "--json"]);
    let failure = failed(failure);
    assert_eq!(failure["error"]["code"], "command_failed");
}

#[test]
fn json_list_does_not_present_stale_cached_instances_as_current() {
    let fixture = Fixture::new();
    fixture.provider(
        json!([{"name":"ice-old", "status":"RUNNING", "zone":"zones/test-a"}]),
        false,
    );
    parsed(fixture.run(&["list", "--cloud", "gcp", "--json"]));
    fixture.provider(json!([]), true);
    let failure = fixture.run(&["list", "--cloud", "gcp", "--json"]);
    failed(failure);
}

#[test]
fn default_list_still_uses_human_output() {
    let fixture = Fixture::new();
    fixture.provider(json!([]), false);
    let output = fixture.run(&["list", "--cloud", "gcp"]);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("No `ice`-managed instances"));
}

#[test]
fn connection_json_remains_parseable_when_readiness_probe_prints_output() {
    let fixture = Fixture::new();
    fixture.provider(
        json!([{
            "name": "ice-example", "status": "RUNNING", "zone": "zones/us-central1-a",
            "machineType": "machineTypes/g2-standard-4",
            "metadata": {"items": [{"key": "ice-workload-kind", "value": "shell"}]}
        }]),
        false,
    );
    let value = parsed(fixture.run(&[
        "shell",
        "--cloud",
        "gcp",
        "ice-example",
        "--print-creds",
        "--json",
    ]));
    assert_eq!(value["command"], "shell");
    assert_eq!(value["result"]["instance"]["id"], "ice-example");
    assert!(
        value["result"]["connect_command"]
            .as_str()
            .unwrap()
            .contains("gcloud")
    );
    assert!(
        value["result"]["connect_command"]
            .as_str()
            .unwrap()
            .contains("ice-example")
    );
}

#[test]
fn local_preview_is_json_without_a_container_runtime() {
    let fixture = Fixture::new();
    let value = parsed(fixture.run(&[
        "create",
        "--cloud",
        "local",
        "--unpack",
        "example-image:tag",
        "--hours",
        "0.25",
        "--dry-run",
        "--json",
    ]));
    assert_eq!(value["result"]["dry_run"], true);
    assert_eq!(value["result"]["requested_hours"], 0.25);
    assert_eq!(value["result"]["workload"]["kind"], "unpack");
    assert!(value["result"]["container_runtime"].is_null());
}

#[test]
fn json_cannot_start_an_interactive_shell() {
    let fixture = Fixture::new();
    let shell = fixture.run(&["shell", "--cloud", "vast.ai", "123", "--json"]);
    assert!(
        failed(shell)["error"]["message"]
            .as_str()
            .unwrap()
            .contains("--print-creds")
    );
}

#[test]
fn global_flag_works_at_every_subcommand_depth_and_config_is_typed_and_redacted() {
    let fixture = Fixture::new();
    let set = parsed(fixture.run(&["--json", "config", "set", "default.gcp.min_cpus=12"]));
    assert_eq!(set["result"]["value"], 12);
    assert!(set["cloud"].is_null());
    let get = parsed(fixture.run(&["config", "--json", "get", "default.gcp.min_cpus"]));
    assert_eq!(get["result"]["value"], 12);
    for key in [
        "auth.vast_ai.api_key",
        "auth.aws.access_key_id",
        "auth.aws.secret_access_key",
    ] {
        let pair = format!("{key}=secret-test-credential");
        let result = parsed(fixture.run(&["config", "set", &pair, "--json"]));
        assert_eq!(result["result"]["value"], "<redacted>");
        assert!(!result.to_string().contains("secret-test-credential"));
    }
    parsed(fixture.run(&["config", "set", "default.runtime_hours=0.123456", "--json"]));
    let result = parsed(fixture.run(&["config", "list", "--json"]));
    assert_eq!(
        result["result"]["values"]["default.runtime_hours"],
        0.123456
    );
    assert!(result["result"]["values"]["default.aws.min_cpus"].is_null());
    assert!(!result.to_string().contains("secret-test-credential"));
    let unset = parsed(fixture.run(&["--json", "config", "unset", "default.gcp.min_cpus"]));
    assert!(unset["result"]["value"].is_null());
    let get = parsed(fixture.run(&["--json", "config", "get", "default.gcp.min_cpus"]));
    assert!(get["result"]["value"].is_null());
}

#[test]
fn argument_and_command_errors_are_json_and_keep_nonzero_exit_status() {
    let fixture = Fixture::new();
    let invalid = failed(fixture.run(&["--json", "start"]));
    assert_eq!(invalid["error"]["code"], "invalid_arguments");
    let invalid = failed(fixture.run(&[
        "config",
        "set",
        "--json",
        "auth.vast_ai.api_key=secret-value",
        "extra",
    ]));
    assert!(!invalid.to_string().contains("secret-value"));
    let unknown = failed(fixture.run(&["config", "get", "unknown.key", "--json"]));
    assert_eq!(unknown["command"], "config get");
    assert_eq!(unknown["error"]["code"], "command_failed");
    failed(fixture.run(&["refresh-catalog", "--cloud", "local", "--json"]));
}

#[test]
fn malformed_config_does_not_echo_credentials_in_json_error() {
    let fixture = Fixture::new();
    let dir = fixture.root.path().join("config/ice");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("config.toml"),
        "[auth.vast_ai]\napi_key = 'secret-token' trailing-garbage",
    )
    .unwrap();
    let error = failed(fixture.run(&["config", "list", "--json"]));
    assert!(!error.to_string().contains("secret-token"));
}

#[test]
fn login_captures_provider_output_and_returns_only_readiness_metadata() {
    let fixture = Fixture::new();
    fixture.script("aws", "#!/bin/sh\nprintf 'raw-provider-identity\\n'\n");
    let value = parsed(fixture.run(&["login", "--cloud", "aws", "--json"]));
    assert_eq!(value["result"]["status"], "ready");
    assert_eq!(value["result"]["method"], "auto_detected");
    assert!(!value.to_string().contains("raw-provider"));
    fixture.runtime();
    let local = parsed(fixture.run(&["--json", "login", "--cloud", "local"]));
    assert_eq!(local["result"]["status"], "ready");
}

#[test]
fn lifecycle_reports_actual_state_and_distinguishes_noop() {
    let fixture = Fixture::new();
    fixture.runtime();
    let args = |command| [command, "--cloud", "local", "ice-example", "--json"];
    let unchanged = parsed(fixture.run(&args("start")));
    assert_eq!(unchanged["result"]["changed"], false);
    let stopped = parsed(fixture.run(&args("stop")));
    assert_eq!(stopped["result"]["changed"], true);
    assert_eq!(stopped["result"]["instance"]["state"], "stopped");
    let stopped = parsed(fixture.run(&args("stop")));
    assert_eq!(stopped["result"]["changed"], false);
    let started = parsed(fixture.run(&args("start")));
    assert_eq!(started["result"]["changed"], true);
    assert_eq!(started["result"]["instance"]["state"], "running");
    let deleted = parsed(fixture.run(&args("delete")));
    assert_eq!(deleted["result"]["status"], "deleted");
    assert_eq!(deleted["result"]["instance_id"], "test-id");
    let actions = fs::read_to_string(fixture.root.path().join("actions")).unwrap();
    assert_eq!(actions.lines().filter(|line| *line == "stop").count(), 2);
    assert_eq!(actions.lines().filter(|line| *line == "start").count(), 1);
    assert_eq!(actions.lines().filter(|line| *line == "rm").count(), 1);
}

#[test]
fn managed_creation_finishes_deployment_before_emitting_json() {
    let fixture = Fixture::new();
    fixture.runtime();
    fixture.script(
        "gcloud",
        r#"#!/bin/sh
if [ "$2" = list ]; then printf 'test@example.invalid\n';
else printf '{"token":"secret-registry-token"}\n'; fi
"#,
    );
    let value = parsed(fixture.run(&[
        "create",
        "--cloud",
        "local",
        "--container",
        "us-central1-docker.pkg.dev/test/repo/image:tag",
        "--hours",
        "0.25",
        "--json",
    ]));
    assert_eq!(value["result"]["status"], "created");
    assert_eq!(value["result"]["instance"]["id"], "test-id");
    assert!(!value.to_string().contains("secret-registry-token"));
    let actions = fs::read_to_string(fixture.root.path().join("actions")).unwrap();
    for action in ["login", "pull", "run", "inspect"] {
        assert!(
            actions.lines().any(|line| line == action),
            "missing {action}: {actions}"
        );
    }
    assert!(!actions.lines().any(|line| matches!(line, "exec" | "logs")));
}

#[test]
fn credential_lookup_errors_do_not_expose_raw_token_responses() {
    let fixture = Fixture::new();
    fixture.runtime();
    fixture.script("gcloud", "#!/bin/sh\nprintf 'secret-registry-token\\n'\n");
    let value = failed(fixture.run(&[
        "create",
        "--cloud",
        "local",
        "--container",
        "us-central1-docker.pkg.dev/test/repo/image:tag",
        "--json",
    ]));
    assert!(!value.to_string().contains("secret-registry-token"));
    assert!(
        value["error"]["message"]
            .as_str()
            .unwrap()
            .contains("registry credentials")
    );
}

#[test]
fn closed_json_pipe_stops_a_log_transport_with_child_processes() {
    use std::io::{BufRead, BufReader, Write};
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    let fixture = Fixture::new();
    fixture.provider(
        json!([{"name":"ice-example", "status":"RUNNING", "zone":"zones/test-a",
        "metadata":{"items":[{"key":"ice-workload-kind","value":"unpack"},
            {"key":"ice-workload-source","value":"example:tag"}]}}]),
        false,
    );
    fixture.script(
        "gcloud",
        r#"#!/bin/sh
if [ "$2" = ssh ]; then
    printf 'first'
    read -r release
    /bin/sh -c 'printf second; exec /bin/sleep 30' &
    wait
else
    /bin/cat "$ICE_TEST_RESPONSE"
fi
"#,
    );
    let mut child = fixture
        .command()
        .args([
            "logs",
            "--cloud",
            "gcp",
            "ice-example",
            "--follow",
            "--json",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&line).unwrap()["result"]["text"],
        "first"
    );
    drop(stdout);
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"continue\n")
        .unwrap();
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(!status.success());
            break;
        }
        if start.elapsed() > Duration::from_secs(5) {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("log transport kept the JSON command alive after stdout closed");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn interrupting_json_logs_stops_the_transport() {
    use std::io::{BufRead, BufReader, Read};
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    let fixture = Fixture::new();
    fixture.provider(
        json!([{"name":"ice-example", "status":"RUNNING", "zone":"zones/test-a",
        "metadata":{"items":[{"key":"ice-workload-kind","value":"unpack"},
            {"key":"ice-workload-source","value":"example:tag"}]}}]),
        false,
    );
    fixture.script(
        "gcloud",
        r#"#!/bin/sh
if [ "$2" = ssh ]; then
    printf 'first'
    /bin/sleep 30 &
    wait
else
    /bin/cat "$ICE_TEST_RESPONSE"
fi
"#,
    );
    let mut child = fixture
        .command()
        .args([
            "logs",
            "--cloud",
            "gcp",
            "ice-example",
            "--follow",
            "--json",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&line).unwrap()["result"]["text"],
        "first"
    );
    // This is the test's own child, which remains unreaped throughout the call.
    assert!(
        Command::new("/bin/kill")
            .args(["-INT", &child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert_eq!(status.code(), Some(130));
            break;
        }
        if start.elapsed() > Duration::from_secs(5) {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("interrupted log transport kept the command alive");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let mut remaining = String::new();
    stdout.read_to_string(&mut remaining).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&remaining).unwrap()["error"]["code"],
        "command_failed"
    );
}

#[test]
fn log_output_is_flushed_before_the_stream_finishes() {
    use std::io::{BufRead, BufReader, Write};
    use std::process::Stdio;
    use std::sync::mpsc;
    use std::time::Duration;

    let fixture = Fixture::new();
    fixture.provider(
        json!([{"name":"ice-example", "status":"RUNNING", "zone":"zones/test-a",
        "metadata":{"items":[{"key":"ice-workload-kind","value":"unpack"},
            {"key":"ice-workload-source","value":"example:tag"}]}}]),
        false,
    );
    // Block on stdin after printing the first chunk. The test releases it only
    // after receiving JSON, proving follow does not buffer until process exit.
    fixture.script(
        "gcloud",
        r#"#!/bin/sh
if [ "$2" = ssh ]; then
    printf 'first chunk'
    read -r release
    printf 'last chunk'
else
    /bin/cat "$ICE_TEST_RESPONSE"
fi
"#,
    );
    let mut child = fixture
        .command()
        .args([
            "logs",
            "--cloud",
            "gcp",
            "ice-example",
            "--follow",
            "--json",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (sender, receiver) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if sender.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    let first = receiver.recv_timeout(Duration::from_secs(10));
    if first.is_err() {
        let _ = child.stdin.take().unwrap().write_all(b"continue\n");
        child.kill().unwrap();
        child.wait().unwrap();
        reader.join().unwrap();
        panic!("JSON stream did not flush its first record");
    }
    let first: Value = serde_json::from_str(&first.unwrap()).unwrap();
    assert_eq!(first["result"]["text"], "first chunk");
    assert!(child.try_wait().unwrap().is_none());
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"continue\n")
        .unwrap();
    assert!(child.wait().unwrap().success());
    reader.join().unwrap();
    let remaining: Vec<Value> = receiver
        .into_iter()
        .map(|line| serde_json::from_str(&line).unwrap())
        .collect();
    assert_eq!(remaining.last().unwrap()["result"]["event"], "complete");
}

#[test]
fn transfers_report_completion_only_after_copy_succeeds() {
    let fixture = Fixture::new();
    let dir = fixture.unpack(b"", None);
    fixture.script("cp", "#!/bin/sh\nexec /bin/cp \"$@\"\n");
    fs::write(dir.join("rootfs/source.txt"), "payload").unwrap();
    let destination = fixture.root.path().join("download.txt");
    let value = parsed(fixture.run(&[
        "pull",
        "--cloud",
        "local",
        "ice-example",
        "source.txt",
        destination.to_str().unwrap(),
        "--json",
    ]));
    assert_eq!(value["result"]["status"], "completed");
    assert_eq!(fs::read_to_string(&destination).unwrap(), "payload");
    let value = parsed(fixture.run(&[
        "--json",
        "push",
        "--cloud",
        "local",
        "ice-example",
        destination.to_str().unwrap(),
        "upload.txt",
    ]));
    assert_eq!(value["result"]["status"], "completed");
    assert_eq!(
        fs::read_to_string(dir.join("rootfs/upload.txt")).unwrap(),
        "payload"
    );
    failed(fixture.run(&[
        "pull",
        "--cloud",
        "local",
        "ice-example",
        "missing.txt",
        "--json",
    ]));
}

#[test]
fn logs_stream_both_process_pipes_as_json_lines() {
    let fixture = Fixture::new();
    fixture.runtime();
    let output = fixture.run(&[
        "logs",
        "--cloud",
        "local",
        "ice-example",
        "--follow",
        "--json",
    ]);
    assert!(output.status.success(), "{:?}", output);
    let records = records(&output.stdout);
    for stream in ["stdout", "stderr"] {
        let text: String = records
            .iter()
            .filter(|record| record["result"]["stream"] == stream)
            .map(|record| record["result"]["text"].as_str().unwrap())
            .collect();
        assert_eq!(
            text,
            if stream == "stdout" {
                "stdout 🦀\n"
            } else {
                "stderr 終"
            }
        );
    }
    assert_eq!(records.last().unwrap()["result"]["event"], "complete");
}

#[test]
fn unpack_logs_include_exit_status_without_invented_text() {
    let fixture = Fixture::new();
    fixture.unpack("first\nlast 🦀".as_bytes(), Some(7));
    let output = fixture.run(&[
        "--json",
        "logs",
        "--cloud",
        "local",
        "ice-example",
        "--tail",
        "1",
        "--follow",
    ]);
    assert!(output.status.success(), "{:?}", output);
    let records = records(&output.stdout);
    let text: String = records
        .iter()
        .filter_map(|record| record["result"]["text"].as_str())
        .collect();
    assert!(text.ends_with("last 🦀"));
    assert_eq!(
        records[records.len() - 2]["result"]["event"],
        "workload_exit"
    );
    assert_eq!(records[records.len() - 2]["result"]["exit_code"], 7);
    assert_eq!(records.last().unwrap()["result"]["event"], "complete");
}

#[test]
fn remote_log_failure_keeps_partial_records_and_ends_with_error() {
    let fixture = Fixture::new();
    fixture.provider(
        json!([{"name":"ice-example", "status":"RUNNING", "zone":"zones/test-a",
        "metadata":{"items":[{"key":"ice-workload-kind","value":"unpack"},
            {"key":"ice-workload-source","value":"example:tag"}]}}]),
        false,
    );
    fixture.script("gcloud", "#!/bin/sh\nif [ \"$2\" = ssh ]; then printf 'partial 🦀'; exit 7; fi\n/bin/cat \"$ICE_TEST_RESPONSE\"\n");
    let output = fixture.run(&["logs", "--cloud", "gcp", "ice-example", "--json"]);
    assert!(!output.status.success());
    let records = records(&output.stdout);
    assert!(
        records
            .iter()
            .any(|record| record["result"]["text"] == "partial 🦀")
    );
    assert_eq!(records.last().unwrap()["error"]["code"], "command_failed");
    assert!(
        !records
            .iter()
            .any(|record| record["result"]["event"] == "complete")
    );
}
