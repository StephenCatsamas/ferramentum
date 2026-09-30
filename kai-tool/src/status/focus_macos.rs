//! Beta macOS activation. Match the session's exact tty, including inactive tabs/panes.
use super::{command, process::ProcessIdentity, process_macos as process};
use anyhow::{Context, Result, bail, ensure};
use std::time::Duration;

pub(super) fn focus(identity: ProcessIdentity) -> Result<()> {
    let chain =
        process::ancestry(identity).context("The session changed. Refresh and try again.")?;
    let args = process::process_arguments(identity.pid)?;
    let environment: Vec<_> = args.environment.split(|byte| *byte == 0).collect();
    ensure!(
        !environment
            .iter()
            .any(|entry| [b"TMUX=".as_slice(), b"STY=", b"ZELLIJ="]
                .iter()
                .any(|key| entry.starts_with(key) && entry.len() > key.len()))
            && !chain
                .iter()
                .any(|ancestor| ancestor.name.starts_with("tmux")
                    || matches!(ancestor.name.as_str(), "screen" | "zellij")),
        "This session needs a multiplexer integration before Kai can select its pane. Press Ctrl+E for terminal details."
    );
    let program = environment
        .iter()
        .find_map(|entry| entry.strip_prefix(b"TERM_PROGRAM="))
        .unwrap_or_default();
    let script = match program {
        b"Apple_Terminal" => include_str!("scripts/terminal.applescript"),
        b"iTerm.app" => include_str!("scripts/iterm.applescript"),
        _ => bail!(
            "Window switching on macOS currently supports Terminal and iTerm2. Press Ctrl+E for terminal details."
        ),
    };
    let mut files = process::files(&[identity.pid])?;
    let tty = files
        .remove(&identity.pid)
        .and_then(|files| files.tty)
        .context("Cannot identify this session's terminal. Press Ctrl+E for terminal details.")?;
    ensure!(
        process::ancestry(identity)? == chain,
        "The session changed. Refresh and try again."
    );
    let reply = command::run("/usr/bin/osascript", &["-e", script, &tty], Duration::from_secs(8))
        .context("Cannot switch terminal windows. Allow terminal automation in System Settings > Privacy & Security > Automation, then try again.")?;
    match reply.trim_ascii() {
        b"focused" => Ok(()),
        b"ambiguous" => bail!(
            "Several terminal sessions share this tty. Press Ctrl+E to locate the session manually."
        ),
        _ => bail!("The terminal session is no longer available. Refresh and try again."),
    }
}
