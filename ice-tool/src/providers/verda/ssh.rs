use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde_json::json;

use super::{Instance, Provider, cleanup_details, client::Client};
use crate::cli::{PullArgs, PushArgs, ShellArgs};
use crate::model::{Cloud, IceConfig};
use crate::providers::{CloudInstance, CloudProvider};
use crate::support::{run_command_status, shell_quote_single};

pub(super) fn identity(config: &IceConfig) -> Result<Option<PathBuf>> {
    crate::support::ensure_command_available("ssh")?;
    if let Some(path) = config.default.verda.ssh_key_path.as_deref() {
        let path = PathBuf::from(path);
        if !path.is_file() {
            bail!("Configured Verda SSH private key path does not exist");
        }
        return Ok(Some(path));
    }
    Ok(crate::remote::discover_local_ssh_keypair()?.map(|(path, _)| path))
}

fn host(instance: &Instance) -> Result<IpAddr> {
    instance
        .ip
        .as_deref()
        .context("Verda instance has no public IP")?
        .parse()
        .context("Verda returned an invalid public IP")
}

fn options(identity: Option<&Path>) -> Vec<String> {
    let mut args = vec![
        "-o".into(),
        "BatchMode=yes".into(),
        "-o".into(),
        "ConnectTimeout=10".into(),
        "-o".into(),
        "ServerAliveInterval=5".into(),
        "-o".into(),
        "ServerAliveCountMax=2".into(),
        "-o".into(),
        "StrictHostKeyChecking=accept-new".into(),
    ];
    if let Some(path) = identity {
        args.extend([
            "-i".into(),
            path.display().to_string(),
            "-o".into(),
            "IdentitiesOnly=yes".into(),
        ]);
    }
    args
}

pub(super) fn connection_command(instance: &Instance, identity: Option<&Path>) -> Result<String> {
    let mut parts = vec!["ssh".to_owned()];
    parts.extend(options(identity));
    parts.push(format!("root@{}", host(instance)?));
    Ok(parts
        .iter()
        .map(|s| shell_quote_single(s))
        .collect::<Vec<_>>()
        .join(" "))
}

pub(super) fn wait_ready(
    instance: &Instance,
    identity: Option<&Path>,
    deadline: Instant,
) -> Result<()> {
    let host = host(instance)?;
    while Instant::now() < deadline {
        let mut command = Command::new("ssh");
        command
            .args(options(identity))
            .arg(format!("root@{host}"))
            .arg("true")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = command
            .spawn()
            .context("Unable to start SSH readiness probe")?;
        let probe_deadline = (Instant::now() + Duration::from_secs(15)).min(deadline);
        loop {
            if let Some(status) = child.try_wait()? {
                if status.success() {
                    return Ok(());
                }
                break;
            }
            if Instant::now() >= probe_deadline {
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        thread::sleep(
            Duration::from_secs(2).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
    Err(crate::automation::error(
        "ssh_not_ready",
        "Verda SSH readiness timed out. Check the registered key, local identity, public IP and guest startup; no key or host configuration was changed.",
        cleanup_details(&instance.id, Some(instance)),
    ))
}

fn resolve(config: &IceConfig, identifier: &str) -> Result<Instance> {
    Provider::resolve_instance(&Client::from_config(config)?, identifier)
}

pub(super) fn shell(config: &IceConfig, args: &ShellArgs) -> Result<()> {
    if args.preserve_ephemeral {
        bail!("--preserve-ephemeral is unsupported on Verda");
    }
    let instance = resolve(config, &args.instance)?;
    if !args.no_probe && !instance.is_running() {
        return Err(crate::automation::error(
            "instance_stopped",
            "Start the Verda instance explicitly before connecting.",
            json!({"instance_id":instance.id,"next_command":format!("ice start --cloud verda {}",instance.id)}),
        ));
    }
    let identity = identity(config)?;
    if !args.no_probe {
        wait_ready(
            &instance,
            identity.as_deref(),
            Instant::now() + crate::automation::startup_timeout(),
        )?;
    }
    if args.print_creds {
        let command = connection_command(&instance, identity.as_deref())?;
        if crate::output::is_json() {
            return crate::output::emit(
                "shell",
                Cloud::Verda,
                json!({"instance":instance.json_summary(), "connect_command":command,
                "readiness":if args.no_probe {"unchecked"} else {"ssh_verified"}, "profiling_access":"unverified"}),
            );
        }
        println!("{command}");
        return Ok(());
    }
    run_command_status(
        Command::new("ssh")
            .args(options(identity.as_deref()))
            .arg(format!("root@{}", host(&instance)?)),
        "Open Verda shell",
    )
}

fn transfer(
    config: &IceConfig,
    instance: &Instance,
    local: &Path,
    remote: &str,
    upload: bool,
) -> Result<()> {
    if !instance.is_running() {
        bail!("Verda instance is not running; start it explicitly");
    }
    let identity = identity(config)?;
    let host = host(instance)?;
    let host = match host {
        IpAddr::V4(ip) => ip.to_string(),
        IpAddr::V6(ip) => format!("[{ip}]"),
    };
    let transport = std::iter::once("ssh".to_owned())
        .chain(options(identity.as_deref()))
        .map(|s| shell_quote_single(&s))
        .collect::<Vec<_>>()
        .join(" ");
    let mut command = Command::new("rsync");
    command.args(["-az", "--protect-args", "-e", &transport, "--"]);
    let remote = format!("root@{host}:{remote}");
    if upload {
        command.arg(local).arg(remote);
    } else {
        command.arg(remote).arg(local);
    }
    if crate::output::is_json() {
        command.stdout(Stdio::null());
    }
    run_command_status(&mut command, "Transfer Verda files")
}

pub(super) fn push(config: &IceConfig, args: &PushArgs) -> Result<()> {
    if !args.local_path.exists() {
        bail!("Upload path does not exist");
    }
    transfer(
        config,
        &resolve(config, &args.instance)?,
        &args.local_path,
        args.remote_path.as_deref().unwrap_or("."),
        true,
    )
}

pub(super) fn pull(config: &IceConfig, args: &PullArgs) -> Result<()> {
    transfer(
        config,
        &resolve(config, &args.instance)?,
        args.local_path.as_deref().unwrap_or(Path::new(".")),
        &args.remote_path,
        false,
    )
}
