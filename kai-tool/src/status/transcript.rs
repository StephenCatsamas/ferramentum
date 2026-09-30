use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashSet,
    fs::OpenOptions,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

const MAX_SCAN_BYTES: u64 = 16 * 1024 * 1024;
const MAX_LINE_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum TurnState {
    Working,
    NeedsInput,
    Ready,
    Interrupted,
    Error,
    #[default]
    Unknown,
    Exited,
}

impl TurnState {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Working => "Active",
            Self::NeedsInput => "Needs input",
            Self::Ready => "Ready",
            Self::Interrupted => "Interrupted",
            Self::Error => "Error",
            Self::Unknown => "Unknown",
            Self::Exited => "Exited",
        }
    }
}

#[derive(Clone, Default)]
pub(super) struct View {
    pub id: String,
    pub cwd: Option<PathBuf>,
    pub state: TurnState,
    pub last_finished_at: Option<i64>,
    pub started_at: Option<i64>,
    pub duration_ms: Option<u64>,
    pub detail: Option<String>,
}

impl View {
    pub(super) fn run_time_ms(&self, now: u64) -> Option<u64> {
        match self.state {
            TurnState::Working | TurnState::NeedsInput => self
                .started_at
                .and_then(|at| u64::try_from(at).ok())
                .map(|at| now.saturating_sub(at).saturating_mul(1000)),
            TurnState::Ready | TurnState::Interrupted | TurnState::Error => self.duration_ms,
            TurnState::Unknown | TurnState::Exited => None,
        }
    }
}

#[derive(Default)]
pub(super) struct Transcript {
    identity: Option<(u64, u64)>,
    offset: u64,
    main: bool,
    view: View,
    turn: Option<String>,
    input_requests: HashSet<String>,
    skipped_line: bool,
}

// Deserialize only observation metadata. Prompts, auth data, tool arguments and output
// are ignored rather than retained in the dashboard's memory or JSON response.
#[derive(Deserialize)]
struct Record {
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(rename = "type")]
    kind: String,
    payload: Payload,
}

#[derive(Default, Deserialize)]
struct Payload {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    source: Option<Value>,
    #[serde(default)]
    cwd: Option<PathBuf>,
    #[serde(default)]
    turn_id: Option<String>,
    #[serde(default)]
    completed_at: Option<i64>,
    #[serde(default)]
    started_at: Option<i64>,
    #[serde(default)]
    duration_ms: Option<i64>,
    #[serde(default)]
    error: Option<Present>,
    #[serde(default)]
    call_id: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

// Avoid retaining or displaying upstream error text, which can include arbitrary content.
struct Present;
impl<'de> Deserialize<'de> for Present {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        serde::de::IgnoredAny::deserialize(deserializer).map(|_| Self)
    }
}

impl Transcript {
    pub(super) fn is_main(&self) -> bool {
        self.main
    }
    pub(super) fn view(&self) -> View {
        self.view.clone()
    }

    pub(super) fn refresh(&mut self, path: &Path) -> Result<()> {
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
            .context("cannot open session log")?;
        let metadata = file.metadata()?;
        // Never follow a renamed log path into another user's file or a special device.
        // SAFETY: geteuid has no preconditions.
        if !metadata.is_file() || metadata.uid() != unsafe { libc::geteuid() } {
            bail!("unsafe session log");
        }
        let identity = (metadata.dev(), metadata.ino());
        if self.identity != Some(identity) || metadata.len() < self.offset {
            *self = Self::default();
            let mut head = Vec::new();
            BufReader::new((&mut file).take(MAX_LINE_BYTES)).read_until(b'\n', &mut head)?;
            if !head.ends_with(b"\n") {
                bail!("incomplete session metadata");
            }
            let record: Record =
                serde_json::from_slice(&head).context("invalid session metadata")?;
            if record.kind != "session_meta" {
                bail!("session log has no metadata header");
            }
            self.main = matches!(
                record.payload.source.as_ref().and_then(Value::as_str),
                Some("cli" | "tui")
            );
            self.view.id = record.payload.id.context("session metadata has no ID")?;
            self.view.cwd = record.payload.cwd;
            self.offset = head.len() as u64;
            self.identity = Some(identity);
        }
        if !self.main {
            return Ok(());
        }
        // Bound startup work for very long conversations and every subsequent refresh.
        // A missing lifecycle marker remains Unknown, never inferred from file activity.
        if metadata.len().saturating_sub(self.offset) > MAX_SCAN_BYTES {
            self.offset = metadata.len() - MAX_SCAN_BYTES;
            file.seek(SeekFrom::Start(self.offset))?;
            let (skipped, complete_line) =
                skip_line(&mut BufReader::new((&mut file).take(MAX_SCAN_BYTES)))?;
            self.offset += skipped;
            self.skipped_line = !complete_line;
            self.turn = None;
            self.input_requests.clear();
            self.view.state = TurnState::Unknown;
            self.view.last_finished_at = None;
            self.view.started_at = None;
            self.view.duration_ms = None;
            self.view.detail =
                Some("Older history omitted; showing available recent turn events".into());
        }
        file.seek(SeekFrom::Start(self.offset))?;
        let mut reader = BufReader::new(file.take(MAX_SCAN_BYTES));
        loop {
            let mut line = Vec::new();
            let count = (&mut reader)
                .take(MAX_LINE_BYTES + 1)
                .read_until(b'\n', &mut line)?;
            if count == 0 {
                break;
            }
            if self.skipped_line || line.len() as u64 > MAX_LINE_BYTES {
                self.offset += count as u64;
                self.skipped_line = !line.ends_with(b"\n");
                self.view.state = TurnState::Unknown;
                self.turn = None;
                self.input_requests.clear();
                self.view.started_at = None;
                self.view.duration_ms = None;
                self.view.detail = Some(
                    "An oversized log record was skipped; waiting for a complete turn event".into(),
                );
                continue;
            }
            if !line.ends_with(b"\n") {
                break;
            } // Retry an unfinished append on the next refresh.
            self.offset += count as u64;
            match serde_json::from_slice::<Record>(&line) {
                Ok(record) => self.apply(record),
                Err(_) => {
                    self.view.state = TurnState::Unknown;
                    self.turn = None;
                    self.input_requests.clear();
                    self.view.started_at = None;
                    self.view.duration_ms = None;
                    self.view.detail = Some(
                        "A log record could not be read; waiting for a complete turn event".into(),
                    );
                }
            }
        }
        Ok(())
    }

    fn apply(&mut self, record: Record) {
        let at = record
            .timestamp
            .as_deref()
            .and_then(|value| OffsetDateTime::parse(value, &Rfc3339).ok())
            .map(OffsetDateTime::unix_timestamp);
        let payload = record.payload;
        match (record.kind.as_str(), payload.kind.as_str()) {
            ("event_msg", "task_started" | "turn_started") => {
                self.turn = payload.turn_id;
                self.input_requests.clear();
                self.view.state = TurnState::Working;
                self.view.started_at = payload.started_at.or(at);
                self.view.duration_ms = None;
                self.view.detail = None;
            }
            ("event_msg", "task_complete" | "turn_complete" | "turn_aborted") => {
                // Ignore a delayed terminal event for an older turn.
                if self.turn.is_some() && payload.turn_id.is_some() && self.turn != payload.turn_id
                {
                    return;
                }
                self.view.state = if payload.kind == "turn_aborted" {
                    TurnState::Interrupted
                } else if payload.error.is_some() {
                    TurnState::Error
                } else {
                    TurnState::Ready
                };
                self.view.last_finished_at = payload.completed_at.or(at);
                self.view.started_at = payload.started_at.or(self.view.started_at);
                self.view.duration_ms = payload
                    .duration_ms
                    .and_then(|value| u64::try_from(value).ok())
                    .or_else(|| {
                        self.view
                            .last_finished_at
                            .zip(self.view.started_at)
                            .and_then(|(end, start)| u64::try_from(end.checked_sub(start)?).ok())
                            .map(|seconds| seconds.saturating_mul(1000))
                    });
                self.view.detail = None;
                self.turn = None;
                self.input_requests.clear();
            }
            ("turn_context", _) => {
                if let Some(cwd) = payload.cwd {
                    self.view.cwd = Some(cwd);
                }
            }
            ("response_item", "function_call")
                if payload.name.as_deref().is_some_and(is_input_request) =>
            {
                if let Some(id) = payload.call_id {
                    self.input_requests.insert(id);
                    self.view.state = TurnState::NeedsInput;
                }
            }
            ("response_item", "function_call_output")
                if payload
                    .call_id
                    .as_ref()
                    .is_some_and(|id| self.input_requests.remove(id))
                    && self.input_requests.is_empty() =>
            {
                self.view.state = if self.turn.is_some() {
                    TurnState::Working
                } else {
                    TurnState::Unknown
                };
            }
            _ => (),
        }
    }
}

fn is_input_request(name: &str) -> bool {
    matches!(name.rsplit('.').next(), Some("request_user_input"))
}

fn skip_line(reader: &mut impl BufRead) -> Result<(u64, bool)> {
    let mut skipped = 0;
    loop {
        let bytes = reader.fill_buf()?;
        if bytes.is_empty() {
            return Ok((skipped, false));
        }
        let newline = bytes.iter().position(|byte| *byte == b'\n');
        let count = newline.map_or(bytes.len(), |index| index + 1);
        reader.consume(count);
        skipped += count as u64;
        if newline.is_some() {
            return Ok((skipped, true));
        }
    }
}

#[cfg(test)]
#[path = "transcript_tests.rs"]
mod tests;
