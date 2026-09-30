use anyhow::{Context, Result};
use std::{
    collections::{BTreeSet, HashMap},
    fs,
    io::{self, Read},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(super) struct ProcessIdentity {
    pub pid: u32,
    // Linux start ticks prevent a reused PID from inheriting another window's state.
    pub start_ticks: u64,
}

pub(super) struct Window {
    pub identity: ProcessIdentity,
    pub tty: Option<String>,
    pub cwd: Option<PathBuf>,
    pub transcripts: Vec<PathBuf>,
    pub warning: Option<String>,
}

struct Process {
    identity: ProcessIdentity,
    parent: u32,
    name: String,
    zombie: bool,
}

pub(super) fn discover(proc_root: &Path, observer_pid: u32) -> Result<(Vec<Window>, Vec<String>)> {
    let owner = fs::metadata(proc_root.join("self"))
        .context("cannot inspect this process in /proc")?
        .uid();
    let mut processes = HashMap::new();
    let mut warnings = Vec::new();
    for entry in fs::read_dir(proc_root).context("cannot enumerate Linux processes")? {
        let entry = entry?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        if pid == observer_pid {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.uid() != owner {
            continue;
        }
        match read_process(&entry.path(), pid) {
            Ok(process) => {
                processes.insert(pid, process);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => (),
            Err(_) => warnings
                .push("Some process metadata could not be read; the list may be incomplete".into()),
        }
    }
    let mut windows = Vec::new();
    for process in processes
        .values()
        .filter(|process| process.name == "kai" && !process.zombie)
    {
        let dir = proc_root.join(process.identity.pid.to_string());
        let command = match read_bounded(&dir.join("cmdline"), 64 * 1024) {
            Ok(command) => command,
            Err(_) => {
                warnings.push(
                    "Some Kai command lines could not be read; the list may be incomplete".into(),
                );
                continue;
            }
        };
        if !is_launch(&command) {
            continue;
        }
        let mut transcripts = BTreeSet::new();
        let mut warning = None;
        for child in processes.values().filter(|child| {
            child.name == "codex" && !child.zombie && child.parent == process.identity.pid
        }) {
            let fd_dir = proc_root.join(child.identity.pid.to_string()).join("fd");
            match fs::read_dir(&fd_dir) {
                Ok(entries) => {
                    for entry in entries.flatten() {
                        let Ok(path) = fs::read_link(entry.path()) else {
                            continue;
                        };
                        if is_rollout(&path) {
                            transcripts.insert(path);
                        }
                    }
                }
                Err(_) => warning = Some("Cannot inspect Codex's open session logs".into()),
            }
            if !matches!(same_process(proc_root, child.identity), Ok(true)) {
                transcripts.clear();
                warning =
                    Some("Codex restarted during observation; waiting for the next refresh".into());
            }
        }
        // The owner may have exited or the PID may have been reused during the scan.
        match same_process(proc_root, process.identity) {
            Ok(false) => continue,
            Err(_) => warning = Some("Cannot verify the window process during this refresh".into()),
            Ok(true) => (),
        }
        windows.push(Window {
            identity: process.identity,
            tty: fs::read_link(dir.join("fd/0")).ok().and_then(|path| {
                let path = path.to_str()?;
                (path.starts_with("/dev/pts/") || path.starts_with("/dev/tty"))
                    .then(|| path.trim_start_matches("/dev/").to_owned())
            }),
            cwd: fs::read_link(dir.join("cwd")).ok(),
            transcripts: transcripts.into_iter().collect(),
            warning,
        });
    }
    Ok((windows, warnings))
}

pub(super) fn same_process(proc_root: &Path, identity: ProcessIdentity) -> io::Result<bool> {
    match read_process(&proc_root.join(identity.pid.to_string()), identity.pid) {
        Ok(process) => Ok(process.identity == identity && !process.zombie),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn read_process(dir: &Path, pid: u32) -> io::Result<Process> {
    let stat = read_bounded(&dir.join("stat"), 16 * 1024)?;
    let stat = std::str::from_utf8(&stat).map_err(|_| io::Error::other("invalid process stat"))?;
    parse_stat(pid, stat).ok_or_else(|| io::Error::other("invalid process stat"))
}

fn parse_stat(pid: u32, stat: &str) -> Option<Process> {
    let start = stat.find('(')?;
    let end = stat.rfind(')')?;
    if end <= start || stat[..start].trim().parse::<u32>().ok()? != pid {
        return None;
    }
    let fields: Vec<_> = stat[end + 1..].split_ascii_whitespace().collect();
    Some(Process {
        identity: ProcessIdentity {
            pid,
            start_ticks: fields.get(19)?.parse().ok()?,
        },
        parent: fields.get(1)?.parse().ok()?,
        name: stat[start + 1..end].to_owned(),
        zombie: matches!(*fields.first()?, "Z" | "X"),
    })
}

fn is_launch(command: &[u8]) -> bool {
    let mut args = command
        .split(|byte| *byte == 0)
        .skip(1)
        .filter(|arg| !arg.is_empty());
    while let Some(arg) = args.next() {
        if arg == b"--credential-provider" {
            args.next();
            continue;
        }
        if arg.starts_with(b"--credential-provider=")
            || matches!(arg, b"--fast" | b"--no-auto-restart")
        {
            continue;
        }
        if arg.starts_with(b"-") {
            return false;
        }
        return b"resume".starts_with(arg);
    }
    true
}

fn is_rollout(path: &Path) -> bool {
    path.is_absolute()
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("rollout-") && name.ends_with(".jsonl"))
}

fn read_bounded(path: &Path, limit: u64) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(io::Error::other("process metadata exceeds size limit"));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_exclude_status_watchers_and_helpers_but_accept_launch_options() {
        for command in [
            b"kai\0".as_slice(),
            b"kai\0r\0id\0",
            b"kai\0--fast\0resume\0",
            b"kai\0--credential-provider\0status\0",
        ] {
            assert!(is_launch(command));
        }
        for command in [
            b"kai\0status\0--watch\0".as_slice(),
            b"kai\0s\0",
            b"kai\0help\0",
            b"kai\0llm\0file\0",
            b"kai\0--version\0",
        ] {
            assert!(!is_launch(command));
        }
    }

    #[test]
    fn stat_parser_handles_parentheses_and_tracks_start_time_and_zombies() {
        let stat = "42 (kai (test)) S 7 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 123 0";
        let process = parse_stat(42, stat).unwrap();
        assert_eq!(process.identity.start_ticks, 123);
        assert_eq!(process.parent, 7);
        assert_eq!(process.name, "kai (test)");
        assert!(!process.zombie);
        assert!(parse_stat(42, &stat.replace(" S ", " Z ")).unwrap().zombie);
        assert!(parse_stat(43, stat).is_none());
        assert!(parse_stat(42, "42 (kai) S").is_none());
    }
}
