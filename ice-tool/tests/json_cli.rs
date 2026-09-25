#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
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
        let path = self.root.path().join("bin/gcloud");
        fs::write(&path, script).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_ice"))
            .args(args)
            .env("PATH", self.root.path().join("bin"))
            .env("XDG_CONFIG_HOME", self.root.path().join("config"))
            .env("XDG_RUNTIME_DIR", self.root.path().join("runtime"))
            .env("ICE_TEST_RESPONSE", self.root.path().join("response.json"))
            .output()
            .unwrap()
    }
}

fn parsed(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "stdout is not a single JSON document: {error}: {}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
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
    assert!(!failure.status.success());
    assert!(failure.stdout.is_empty());
    assert!(!failure.stderr.is_empty());
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
    assert!(!failure.status.success());
    assert!(failure.stdout.is_empty());
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
fn json_cannot_start_an_interactive_shell_or_managed_workload() {
    let fixture = Fixture::new();
    let shell = fixture.run(&["shell", "--cloud", "vast.ai", "123", "--json"]);
    assert!(!shell.status.success());
    assert!(shell.stdout.is_empty());
    assert!(String::from_utf8_lossy(&shell.stderr).contains("--print-creds"));
    let create = fixture.run(&[
        "create",
        "--cloud",
        "vast.ai",
        "--unpack",
        "example-image:tag",
        "--json",
    ]);
    assert!(!create.status.success());
    assert!(create.stdout.is_empty());
    assert!(String::from_utf8_lossy(&create.stderr).contains("requires `--ssh` or `--dry-run`"));
}
