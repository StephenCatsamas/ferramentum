//! Read-only local observation. A running process and a completed turn are separate facts.

#[cfg(target_os = "linux")]
mod process;
#[cfg(target_os = "linux")]
mod transcript;

use anyhow::{Result, bail};
use clap::Args;

#[derive(Debug, Args)]
pub(crate) struct StatusArgs {
    /// Refresh continuously; press q, Esc, or Ctrl-C to leave the terminal view.
    #[arg(long)]
    watch: bool,
    /// Print JSON; with --watch, emit one snapshot per line.
    #[arg(long)]
    json: bool,
    /// Seconds between refreshes in watch mode.
    #[arg(long, default_value_t = 2, value_parser = clap::value_parser!(u64).range(1..=3600))]
    interval: u64,
}

pub(crate) fn run(args: StatusArgs) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        let result = linux::run(args);
        if result.as_ref().err().is_some_and(|error| {
            error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::BrokenPipe)
                || error
                    .downcast_ref::<serde_json::Error>()
                    .is_some_and(|error| {
                        error.io_error_kind() == Some(std::io::ErrorKind::BrokenPipe)
                    })
        }) {
            return Ok(());
        }
        result
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = args;
        bail!("kai status currently requires Linux /proc process inspection")
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::process::{ProcessIdentity, Window, discover, same_process};
    use super::transcript::{Transcript, TurnState};
    use super::*;
    use crossterm::{
        cursor,
        event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
        execute, terminal,
    };
    use serde::Serialize;
    use std::{
        collections::{BTreeMap, HashMap, HashSet},
        io::{self, IsTerminal, Write},
        path::{Path, PathBuf},
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };
    use unicode_width::UnicodeWidthChar;

    const EXIT_RETENTION_SECONDS: u64 = 600;

    #[derive(Clone, Serialize)]
    struct Row {
        pid: u32,
        tty: Option<String>,
        cwd: Option<String>,
        thread_id: Option<String>,
        state: TurnState,
        last_finished_at: Option<i64>,
        exited_at: Option<u64>,
        detail: Option<String>,
    }

    #[derive(Serialize)]
    struct Snapshot {
        version: u8,
        observed_at: u64,
        windows: Vec<Row>,
        warnings: Vec<String>,
    }

    #[derive(Default)]
    struct Observer {
        transcripts: HashMap<PathBuf, Transcript>,
        windows: BTreeMap<ProcessIdentity, Row>,
    }

    impl Observer {
        fn snapshot(&mut self, now: u64) -> Result<Snapshot> {
            let (windows, warnings) = discover(Path::new("/proc"), std::process::id())?;
            Ok(self.observe(windows, warnings, now, |identity| {
                same_process(Path::new("/proc"), identity)
            }))
        }

        fn observe(
            &mut self,
            windows: Vec<Window>,
            mut warnings: Vec<String>,
            now: u64,
            mut still_alive: impl FnMut(ProcessIdentity) -> io::Result<bool>,
        ) -> Snapshot {
            let mut live = HashSet::new();
            let mut paths = HashSet::new();
            for window in windows {
                live.insert(window.identity);
                let mut roots = Vec::new();
                let mut errors = Vec::new();
                for path in &window.transcripts {
                    paths.insert(path.clone());
                    let transcript = self.transcripts.entry(path.clone()).or_default();
                    match transcript.refresh(path) {
                        Ok(()) if transcript.is_main() => roots.push(transcript.view()),
                        Ok(()) => (),
                        Err(_) => errors.push("Could not read a session log"),
                    }
                }
                // A process can retain multiple root threads after switching conversations.
                // Without a UI attachment signal, do not guess which one is on screen.
                let mut row = Row {
                    pid: window.identity.pid,
                    tty: window.tty,
                    cwd: window.cwd.map(|path| path.to_string_lossy().into_owned()),
                    thread_id: None,
                    state: TurnState::Unknown,
                    last_finished_at: None,
                    exited_at: None,
                    detail: None,
                };
                match roots.as_slice() {
                    [root] if errors.is_empty() => {
                        row.thread_id = Some(root.id.clone());
                        row.cwd = root
                            .cwd
                            .as_ref()
                            .map(|path| path.to_string_lossy().into_owned())
                            .or(row.cwd);
                        row.state = root.state;
                        row.last_finished_at = root.last_finished_at;
                        row.detail.clone_from(&root.detail);
                    }
                    [] => row.detail = Some(
                        "No readable main session log (starting, recovering, or unsupported build)"
                            .into(),
                    ),
                    [_] => row.detail = Some("Some session logs could not be read".into()),
                    _ => {
                        row.detail =
                            Some("Multiple main conversations are open in this process".into())
                    }
                }
                if row.thread_id.is_none()
                    && let Some(previous) = self.windows.get(&window.identity)
                {
                    row.thread_id.clone_from(&previous.thread_id);
                    row.last_finished_at = previous.last_finished_at;
                }
                if let Some(warning) = window.warning {
                    row.state = TurnState::Unknown;
                    row.detail = Some(warning);
                }
                self.windows.insert(window.identity, row);
            }
            for (identity, row) in &mut self.windows {
                if !live.contains(identity) && row.exited_at.is_none() {
                    if matches!(still_alive(*identity), Ok(false)) {
                        row.exited_at = Some(now);
                        row.state = TurnState::Exited;
                        row.detail =
                            Some("Window process exited; exit result is not available".into());
                    } else {
                        row.state = TurnState::Unknown;
                        row.detail =
                            Some("Window could not be inspected during this refresh".into());
                    }
                }
            }
            self.windows.retain(|_, row| {
                row.exited_at
                    .is_none_or(|at| now.saturating_sub(at) < EXIT_RETENTION_SECONDS)
            });
            self.transcripts.retain(|path, _| paths.contains(path));
            let mut rows: Vec<_> = self.windows.values().cloned().collect();
            rows.sort_by_key(|row| {
                (
                    row.exited_at.is_some(),
                    state_order(row.state),
                    std::cmp::Reverse(row.last_finished_at),
                    row.pid,
                )
            });
            warnings.sort();
            warnings.dedup();
            Snapshot {
                version: 1,
                observed_at: now,
                windows: rows,
                warnings,
            }
        }
    }

    fn state_order(state: TurnState) -> u8 {
        match state {
            TurnState::NeedsInput => 0,
            TurnState::Working => 1,
            TurnState::Ready => 2,
            TurnState::Interrupted | TurnState::Error => 3,
            TurnState::Unknown => 4,
            TurnState::Exited => 5,
        }
    }

    pub(super) fn run(args: StatusArgs) -> Result<()> {
        if args.watch && !args.json && (!io::stdout().is_terminal() || !io::stdin().is_terminal()) {
            bail!(
                "kai status --watch requires a terminal; use --watch --json for streaming output"
            );
        }
        let _terminal = (args.watch && !args.json).then(Screen::enter).transpose()?;
        let mut observer = Observer::default();
        loop {
            let started = Instant::now();
            let snapshot = observer.snapshot(now())?;
            if args.json {
                let mut stdout = io::stdout().lock();
                serde_json::to_writer(&mut stdout, &snapshot)?;
                writeln!(stdout)?;
                stdout.flush()?;
            } else {
                let size = io::stdout()
                    .is_terminal()
                    .then(terminal::size)
                    .transpose()?;
                let text = render(&snapshot, size, args.watch);
                let mut stdout = io::stdout().lock();
                if args.watch {
                    execute!(
                        stdout,
                        cursor::MoveTo(0, 0),
                        terminal::Clear(terminal::ClearType::All)
                    )?;
                }
                write!(stdout, "{text}")?;
                stdout.flush()?;
            }
            if !args.watch {
                break;
            }
            let delay = Duration::from_secs(args.interval).saturating_sub(started.elapsed());
            if args.json {
                std::thread::sleep(delay);
            } else if quit_requested(delay)? {
                break;
            }
        }
        Ok(())
    }

    fn now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    fn quit_requested(delay: Duration) -> Result<bool> {
        let deadline = Instant::now() + delay;
        while event::poll(deadline.saturating_duration_since(Instant::now()))? {
            if let Event::Key(key) = event::read()?
                && key.kind != KeyEventKind::Release
                && (matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
                    || (key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL)))
            {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                break;
            }
        }
        Ok(false)
    }

    struct Screen;
    impl Screen {
        fn enter() -> Result<Self> {
            terminal::enable_raw_mode()?;
            let screen = Self;
            execute!(io::stdout(), terminal::EnterAlternateScreen, cursor::Hide)?;
            Ok(screen)
        }
    }
    impl Drop for Screen {
        fn drop(&mut self) {
            let _ = execute!(io::stdout(), cursor::Show, terminal::LeaveAlternateScreen);
            let _ = terminal::disable_raw_mode();
        }
    }

    fn render(snapshot: &Snapshot, size: Option<(u16, u16)>, watch: bool) -> String {
        let width = size.map_or(usize::MAX, |(width, _)| {
            usize::from(width).saturating_sub(1)
        });
        let mut lines = vec![format!(
            "Kai windows: {} open{}",
            snapshot
                .windows
                .iter()
                .filter(|row| row.exited_at.is_none())
                .count(),
            if watch {
                "  |  q / Esc / Ctrl-C to quit"
            } else {
                ""
            }
        )];
        lines.push(format!(
            "{:<11} {:<8} {:<9} {:<14} {}",
            "STATE", "PID", "TTY", "LAST FINISHED", "DIRECTORY / SESSION"
        ));
        if snapshot.windows.is_empty() {
            lines.push("No Kai windows found for this user.".into());
        }
        for row in &snapshot.windows {
            let finished = row
                .last_finished_at
                .map_or_else(|| "—".into(), |at| age(snapshot.observed_at, at));
            lines.push(format!(
                "{:<11} {:<8} {:<9} {:<14} {}  {}",
                row.state.label(),
                row.pid,
                row.tty.as_deref().unwrap_or("—"),
                finished,
                row.cwd.as_deref().unwrap_or("?"),
                row.thread_id.as_deref().unwrap_or("")
            ));
            if let Some(detail) = &row.detail {
                lines.push(format!("  {detail}"));
            }
        }
        for warning in &snapshot.warnings {
            lines.push(format!("Warning: {warning}"));
        }
        lines.push("Finished = last recorded turn ended; Ready windows remain open. Updates may lag log writes.".into());
        if let Some((_, height)) = size.filter(|_| watch) {
            let max_lines = usize::from(height).saturating_sub(1).max(1);
            if lines.len() > max_lines {
                let hidden = lines.len() - max_lines + 1;
                lines.truncate(max_lines.saturating_sub(1));
                lines.push(format!(
                    "… {hidden} more lines; enlarge the terminal or use kai status --json"
                ));
            }
        }
        let newline = if watch { "\r\n" } else { "\n" };
        lines
            .into_iter()
            .map(|line| format!("{}{newline}", safe_text(&line, width)))
            .collect()
    }

    fn age(now: u64, at: i64) -> String {
        let seconds = now.saturating_sub(u64::try_from(at).unwrap_or_default());
        if seconds < 60 {
            format!("{seconds}s ago")
        } else if seconds < 3600 {
            format!("{}m ago", seconds / 60)
        } else if seconds < 86_400 {
            format!("{}h {}m ago", seconds / 3600, seconds % 3600 / 60)
        } else {
            format!("{}d ago", seconds / 86_400)
        }
    }

    fn safe_text(value: &str, max_width: usize) -> String {
        let mut width = 0;
        value
            .chars()
            .filter(|ch| {
                !ch.is_control() && !matches!(ch, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
            })
            .take_while(|ch| {
                width += ch.width().unwrap_or_default();
                width <= max_width
            })
            .collect()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn window(start_ticks: u64) -> Window {
            Window {
                identity: ProcessIdentity {
                    pid: 42,
                    start_ticks,
                },
                tty: None,
                cwd: None,
                transcripts: Vec::new(),
                warning: None,
            }
        }

        #[test]
        fn exits_require_evidence_and_pid_reuse_does_not_inherit_state() {
            let mut observer = Observer::default();
            observer.observe(vec![window(1)], vec![], 100, |_| Ok(true));
            let unavailable = observer.observe(vec![], vec![], 101, |_| {
                Err(io::ErrorKind::PermissionDenied.into())
            });
            assert_eq!(unavailable.windows[0].state, TurnState::Unknown);
            assert_eq!(unavailable.windows[0].exited_at, None);
            let reused = observer.observe(vec![window(2)], vec![], 102, |_| Ok(false));
            assert_eq!(reused.windows.len(), 2);
            assert_eq!(reused.windows[0].state, TurnState::Unknown);
            assert_eq!(reused.windows[1].state, TurnState::Exited);
            let expired = observer.observe(vec![window(2)], vec![], 702, |_| Ok(true));
            assert_eq!(expired.windows.len(), 1);
        }

        #[test]
        fn rendering_removes_terminal_controls_and_respects_cell_width() {
            assert_eq!(safe_text("a\n\u{1b}[31m\u{202e}b", 99), "a[31mb");
            assert_eq!(safe_text("a界b", 3), "a界");
            let snapshot = Snapshot {
                version: 1,
                observed_at: 100,
                windows: vec![],
                warnings: vec![],
            };
            let text = render(&snapshot, Some((20, 3)), true);
            assert_eq!(text.lines().count(), 2);
            assert!(
                text.lines()
                    .all(|line| unicode_width::UnicodeWidthStr::width(line) <= 19)
            );
        }
    }
}
