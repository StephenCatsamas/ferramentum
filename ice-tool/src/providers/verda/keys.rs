//! Match existing local/agent keys to provider public keys; never register or edit keys.
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::Result;
use serde_json::json;

use super::{Instance, SshKey, client::Client, resource_path};
use crate::model::IceConfig;
use crate::ssh_probe::ProbeChild;

fn public_key(value: &str) -> Option<(&str, &str)> {
    let mut fields = value.split_whitespace();
    let kind = fields.next()?;
    if !(kind.starts_with("ssh-") || kind.starts_with("ecdsa-") || kind.starts_with("sk-")) {
        return None;
    }
    Some((kind, fields.next()?))
}

fn candidate_paths(configured: Option<&Path>, directory: Option<&Path>) -> Result<Vec<PathBuf>> {
    if let Some(path) = configured {
        anyhow::ensure!(
            path.is_file(),
            "Configured Verda SSH private key path does not exist"
        );
        return Ok(vec![path.to_owned()]);
    }
    let mut paths = Vec::new();
    if let Some(directory) = directory.filter(|dir| dir.is_dir()) {
        for entry in fs::read_dir(directory)? {
            let path = entry?.path();
            if path.extension().is_some_and(|ext| ext == "pub") {
                let private = path.with_extension("");
                if private.is_file() {
                    paths.push(private);
                }
            }
        }
        for name in [
            "id_ed25519",
            "id_rsa",
            "id_ecdsa",
            "id_ed25519_sk",
            "id_ecdsa_sk",
        ] {
            let path = directory.join(name);
            if path.is_file() && !paths.contains(&path) {
                paths.push(path);
            }
        }
    }
    paths.sort();
    Ok(paths)
}

fn local_output(command: &mut Command, deadline: Instant) -> Result<Option<String>> {
    let cancellation = capulus::Cancellation::install()?;
    cancellation.check()?;
    if Instant::now() >= deadline {
        return Ok(None);
    }
    let mut output = tempfile::NamedTempFile::new()?;
    command
        .stdin(Stdio::null())
        .stdout(output.reopen()?)
        .stderr(Stdio::null())
        .env("SSH_ASKPASS_REQUIRE", "never")
        .env("LC_ALL", "C");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = match command.spawn() {
        Ok(child) => ProbeChild(child),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    loop {
        cancellation.check()?;
        if let Some(status) = child.0.try_wait()? {
            if !status.success() || output.as_file().metadata()?.len() > 64 * 1024 {
                return Ok(None);
            }
            let mut text = String::new();
            output
                .as_file_mut()
                .take(64 * 1024)
                .read_to_string(&mut text)?;
            return Ok(Some(text));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        cancellation.sleep(
            Duration::from_millis(20).min(deadline.saturating_duration_since(Instant::now())),
        )?;
    }
}

fn choose_identity(
    registered: &[SshKey],
    paths: &[PathBuf],
    configured: bool,
    mut derive_public: impl FnMut(&Path) -> Result<Option<String>>,
    agent_keys: impl FnOnce() -> Result<Option<String>>,
) -> Result<Option<PathBuf>> {
    let keys = registered
        .iter()
        .filter_map(|key| key.key.as_deref().and_then(public_key))
        .collect::<Vec<_>>();
    anyhow::ensure!(
        !keys.is_empty(),
        "Verda SSH key metadata omitted usable public keys; inspect the selected key IDs"
    );
    let matches = |text: &str| {
        text.lines()
            .filter_map(public_key)
            .any(|key| keys.contains(&key))
    };
    let mut encrypted_candidate = None;
    for path in paths {
        if let Some(public) = derive_public(path)? {
            if matches(&public) {
                return Ok(Some(path.clone()));
            }
        } else {
            // An encrypted/hardware key can still be used through ssh-agent.
            // In that case require its public counterpart as well as an agent match.
            let mut public_path = path.as_os_str().to_os_string();
            public_path.push(".pub");
            if let Ok(public) = fs::read_to_string(PathBuf::from(public_path))
                && matches(&public)
            {
                encrypted_candidate = Some((path.clone(), public));
            }
        }
    }
    if let Some(agent) = agent_keys()? {
        if let Some((path, public)) = encrypted_candidate
            && agent
                .lines()
                .filter_map(public_key)
                .any(|key| Some(key) == public_key(&public))
        {
            return Ok(Some(path));
        }
        if !configured && matches(&agent) {
            return Ok(None);
        }
    }
    Err(crate::automation::error(
        "ssh_key_mismatch",
        "No usable local SSH identity matches the selected Verda public key. Set default.verda.ssh_key_path to its private key; load encrypted keys with ssh-add. This check did not change any instance or key.",
        json!({"ssh_key_ids":registered.iter().map(|key| &key.id).collect::<Vec<_>>(), "configured_identity":configured, "key_changed":false}),
    ))
}

pub(super) fn identity(config: &IceConfig, registered: &[SshKey]) -> Result<Option<PathBuf>> {
    crate::support::ensure_command_available("ssh")?;
    let configured = config.default.verda.ssh_key_path.as_deref().map(Path::new);
    let directory = dirs::home_dir().map(|home| home.join(".ssh"));
    let paths = candidate_paths(configured, directory.as_deref())?;
    let deadline = Instant::now() + Duration::from_secs(10);
    choose_identity(
        registered,
        &paths,
        configured.is_some(),
        |path| {
            local_output(
                Command::new("ssh-keygen")
                    .args(["-y", "-P", "", "-f"])
                    .arg(path),
                deadline,
            )
        },
        || local_output(Command::new("ssh-add").arg("-L"), deadline),
    )
}

pub(super) fn registered_key(client: &Client, id: &str) -> Result<SshKey> {
    let key: SshKey = client.get(&resource_path("ssh-keys", id)?, Duration::from_secs(30))?;
    anyhow::ensure!(
        key.id == id,
        "Verda returned a different SSH key than requested"
    );
    Ok(key)
}

pub(super) fn instance_identity(
    client: &Client,
    config: &IceConfig,
    instance: &Instance,
) -> Result<Option<PathBuf>> {
    let ids = if instance.ssh_key_ids.is_empty() {
        config
            .default
            .verda
            .ssh_key_id
            .iter()
            .cloned()
            .collect::<Vec<_>>()
    } else {
        instance.ssh_key_ids.clone()
    };
    if ids.is_empty() {
        return super::ssh::identity(config);
    }
    let registered = ids
        .iter()
        .map(|id| registered_key(client, id))
        .collect::<Result<Vec<_>>>()?;
    identity(config, &registered)
}

#[cfg(test)]
mod tests;
