//! Session-picker conventions used by `kai r`: compact chrome, blue selection,
//! type-to-search, arrow navigation, Ctrl-O density, and a ruled shortcut footer.
use super::linux::{Observer, Row, Snapshot, age, now, safe_text};
use super::process::ProcessIdentity;
use super::transcript::TurnState;
use anyhow::Result;
use crossterm::{
    cursor,
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers,
    },
    execute,
    style::{Attribute, ResetColor, SetAttribute},
    terminal,
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{List, ListItem, ListState, Paragraph},
};
use std::{
    io,
    time::{Duration, Instant},
};
use unicode_width::UnicodeWidthStr;

pub(super) fn watch(interval: Duration) -> Result<()> {
    let _screen = Screen::enter()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let mut observer = Observer::default();
    let mut snapshot = observer.snapshot(now())?;
    let mut view = View::new(Palette::from_env());
    view.reconcile(&snapshot);
    let mut next_refresh = Instant::now() + interval;
    let mut dirty = true;
    loop {
        if Instant::now() >= next_refresh {
            snapshot = observer.snapshot(now())?;
            view.reconcile(&snapshot);
            next_refresh = Instant::now() + interval;
            dirty = true;
        }
        if dirty {
            terminal.draw(|frame| view.draw(frame, &snapshot, interval))?;
            dirty = false;
        }
        if event::poll(next_refresh.saturating_duration_since(Instant::now()))? {
            match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => {
                    if view.key(key, &snapshot) {
                        break;
                    }
                    dirty = true;
                }
                Event::Paste(text) => {
                    view.add_query(&text);
                    view.reconcile(&snapshot);
                    dirty = true;
                }
                Event::Resize(..) => dirty = true,
                _ => (),
            }
        }
    }
    Ok(())
}

struct Screen;
impl Screen {
    fn enter() -> Result<Self> {
        terminal::enable_raw_mode()?;
        let screen = Self;
        execute!(
            io::stdout(),
            terminal::EnterAlternateScreen,
            EnableBracketedPaste,
            cursor::Hide
        )?;
        Ok(screen)
    }
}
impl Drop for Screen {
    fn drop(&mut self) {
        let _ = execute!(
            io::stdout(),
            DisableBracketedPaste,
            SetAttribute(Attribute::Reset),
            ResetColor,
            cursor::Show,
            terminal::LeaveAlternateScreen
        );
        let _ = terminal::disable_raw_mode();
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Filter {
    #[default]
    All,
    Working,
    Finished,
}
impl Filter {
    const ALL: [Self; 3] = [Self::All, Self::Working, Self::Finished];
    fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Working => "Working",
            Self::Finished => "Finished",
        }
    }
    fn includes(self, row: &Row) -> bool {
        match self {
            Self::All => true,
            Self::Working => matches!(row.state, TurnState::Working | TurnState::NeedsInput),
            Self::Finished => matches!(
                row.state,
                TurnState::Ready | TurnState::Interrupted | TurnState::Error | TurnState::Exited
            ),
        }
    }
    fn cycle(self, backwards: bool) -> Self {
        let index = Self::ALL
            .iter()
            .position(|filter| *filter == self)
            .unwrap_or_default();
        Self::ALL[(index + if backwards { 2 } else { 1 }) % 3]
    }
}

#[derive(Clone, Copy)]
struct Palette {
    selection: Style,
    secondary: Style,
    colors: bool,
}
impl Palette {
    fn from_env() -> Self {
        let term = std::env::var("TERM").unwrap_or_default();
        let colors =
            !std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty()) && term != "dumb";
        let truecolor = matches!(
            std::env::var("COLORTERM").as_deref(),
            Ok("truecolor" | "24bit")
        );
        let selection = if !colors {
            Style::new().reversed().bold()
        } else if truecolor {
            Style::new()
                .fg(Color::Rgb(0, 0, 46))
                .bg(Color::Rgb(99, 168, 248))
                .bold()
        } else if term.contains("256color") {
            Style::new()
                .fg(Color::Indexed(17))
                .bg(Color::Indexed(75))
                .bold()
        } else {
            Style::new().black().on_cyan().bold()
        };
        Self {
            selection,
            secondary: Style::new().dim(),
            colors,
        }
    }
    fn state(self, state: TurnState) -> Style {
        if !self.colors {
            return Style::new().bold();
        }
        let color = match state {
            TurnState::Working => Color::Cyan,
            TurnState::Ready => Color::Green,
            TurnState::NeedsInput | TurnState::Interrupted => Color::Yellow,
            TurnState::Error => Color::Red,
            TurnState::Unknown | TurnState::Exited => Color::Reset,
        };
        Style::new().fg(color).bold()
    }
}

struct View {
    query: String,
    filter: Filter,
    dense: bool,
    expanded: bool,
    selected: Option<ProcessIdentity>,
    visible: Vec<usize>,
    list: ListState,
    page_size: usize,
    palette: Palette,
}

impl View {
    fn new(palette: Palette) -> Self {
        Self {
            query: String::new(),
            filter: Filter::All,
            dense: true,
            expanded: false,
            selected: None,
            visible: Vec::new(),
            list: ListState::default(),
            page_size: 1,
            palette,
        }
    }

    fn reconcile(&mut self, snapshot: &Snapshot) {
        let query = self.query.to_lowercase();
        self.visible = snapshot
            .windows
            .iter()
            .enumerate()
            .filter(|(_, row)| {
                self.filter.includes(row)
                    && (query.is_empty()
                        || [
                            row.thread_name.as_deref().unwrap_or("Unnamed thread"),
                            row.cwd.as_deref().unwrap_or_default(),
                            row.thread_id.as_deref().unwrap_or_default(),
                            row.tty.as_deref().unwrap_or_default(),
                            row.state.label(),
                            &row.pid.to_string(),
                        ]
                        .iter()
                        .any(|value| value.to_lowercase().contains(&query)))
            })
            .map(|(index, _)| index)
            .collect();
        let index = self
            .visible
            .iter()
            .position(|index| Some(snapshot.windows[*index].identity) == self.selected)
            .unwrap_or_else(|| {
                self.list
                    .selected()
                    .unwrap_or_default()
                    .min(self.visible.len().saturating_sub(1))
            });
        self.select(index, snapshot);
    }

    fn select(&mut self, index: usize, snapshot: &Snapshot) {
        self.selected = self
            .visible
            .get(index)
            .map(|index| snapshot.windows[*index].identity);
        self.list.select(self.selected.map(|_| index));
    }

    fn add_query(&mut self, text: &str) {
        let clean = safe_text(text, 1024);
        self.query.extend(
            clean
                .chars()
                .take(1024_usize.saturating_sub(self.query.chars().count())),
        );
    }

    fn key(&mut self, key: KeyEvent, snapshot: &Snapshot) -> bool {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        let index = self.list.selected().unwrap_or_default();
        let last = self.visible.len().saturating_sub(1);
        match key.code {
            KeyCode::Char('c') if control => return true,
            KeyCode::Esc if self.query.is_empty() => return true,
            KeyCode::Esc => self.query.clear(),
            KeyCode::Char('o') if control => self.dense = !self.dense,
            KeyCode::Char('e') if control => self.expanded = !self.expanded,
            KeyCode::Char('u') if control => self.query.clear(),
            KeyCode::Up => self.select(index.saturating_sub(1), snapshot),
            KeyCode::Down => self.select(index.saturating_add(1).min(last), snapshot),
            KeyCode::PageUp => self.select(index.saturating_sub(self.page_size), snapshot),
            KeyCode::PageDown => {
                self.select(index.saturating_add(self.page_size).min(last), snapshot)
            }
            KeyCode::Home => self.select(0, snapshot),
            KeyCode::End => self.select(last, snapshot),
            KeyCode::Left | KeyCode::Right => {
                self.filter = self.filter.cycle(key.code == KeyCode::Left)
            }
            KeyCode::Backspace => {
                self.query.pop();
            }
            KeyCode::Char(ch) if !control && !key.modifiers.contains(KeyModifiers::ALT) => {
                self.add_query(&ch.to_string())
            }
            _ => (),
        }
        self.reconcile(snapshot);
        false
    }

    fn draw(&mut self, frame: &mut Frame, snapshot: &Snapshot, interval: Duration) {
        let area = frame.area();
        // Match the resume picker's collapsing vertical gaps and one-column chrome inset.
        let gap = u16::from(area.height >= 18);
        let [header, _, toolbar, _, search, columns, list, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(gap),
            Constraint::Length(1),
            Constraint::Length(gap),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(3),
        ])
        .areas(area);
        let inset = |rect: Rect| {
            Rect::new(
                rect.x.saturating_add(1),
                rect.y,
                rect.width.saturating_sub(2),
                rect.height,
            )
        };
        let header_text = format!(
            "Kai windows  ·  {} open",
            snapshot
                .windows
                .iter()
                .filter(|row| row.exited_at.is_none())
                .count()
        );
        frame.render_widget(Line::from(header_text).bold(), inset(header));
        let mut tabs = vec![Span::styled("Filter: ", self.palette.secondary)];
        for filter in Filter::ALL {
            let style = if self.filter == filter {
                self.palette.selection
            } else {
                self.palette.secondary
            };
            tabs.push(Span::styled(format!(" {} ", filter.label()), style));
        }
        if toolbar.width >= 62 {
            tabs.push(Span::styled(
                format!("   Refresh {}s", interval.as_secs()),
                self.palette.secondary,
            ));
        }
        frame.render_widget(Line::from(tabs), inset(toolbar));
        let search_text = if self.query.is_empty() {
            "Type to search".to_owned()
        } else {
            format!("Search: {}", self.query)
        };
        frame.render_widget(
            Line::from(clip(&search_text, inset(search).width.into()))
                .style(self.palette.secondary),
            inset(search),
        );
        let compact = area.width < 62;
        let cols = Columns::new(area.width);
        let last_finished = if cols.finished > 0 {
            format!("{:width$}", "Last finished", width = cols.finished)
        } else {
            String::new()
        };
        frame.render_widget(
            Line::from(format!(
                "  {:state_width$}{:time_width$}{last_finished}Thread",
                "State",
                "Run time",
                state_width = cols.state,
                time_width = cols.time
            ))
            .style(self.palette.secondary),
            inset(columns),
        );

        let list = inset(list);
        self.page_size = (usize::from(list.height) / if self.dense { 1 } else { 3 }).max(1);
        if self.visible.is_empty() {
            let message = if snapshot.windows.is_empty() {
                "No Kai windows"
            } else {
                "No matching windows"
            };
            frame.render_widget(Line::from(message).style(self.palette.secondary), list);
        } else {
            let items = self
                .visible
                .iter()
                .enumerate()
                .map(|(visible_index, index)| {
                    let row = &snapshot.windows[*index];
                    self.item(
                        row,
                        snapshot.observed_at,
                        list.width.saturating_sub(2),
                        cols,
                        visible_index,
                    )
                })
                .collect::<Vec<_>>();
            frame.render_stateful_widget(
                List::new(items)
                    .highlight_symbol("› ")
                    .highlight_style(self.palette.selection),
                list,
                &mut self.list,
            );
        }
        if footer.height > 0 {
            let rule = Rect::new(footer.x, footer.y, footer.width, 1);
            frame.render_widget(
                Line::from("─".repeat(footer.width.into())).style(self.palette.secondary),
                rule,
            );
            let position = self.list.selected().map_or(0, |index| index + 1);
            let progress = format!(" {position} / {} ", self.visible.len());
            let width = progress.width() as u16;
            if width < rule.width {
                frame.render_widget(
                    Line::from(progress).style(self.palette.secondary),
                    Rect::new(rule.right() - width - 1, rule.y, width, 1),
                );
            }
            let quit = if self.query.is_empty() {
                "quit"
            } else {
                "clear search"
            };
            let density = if self.dense { "comfortable" } else { "dense" };
            let rows = if compact {
                vec![
                    hints(
                        &[("esc", quit), ("↑/↓", "browse"), ("ctrl+c", "quit")],
                        self.palette.secondary,
                        inset(footer).width,
                    ),
                    hints(
                        &[("ctrl+o", density), ("ctrl+e", "details")],
                        self.palette.secondary,
                        inset(footer).width,
                    ),
                ]
            } else {
                vec![
                    hints(
                        &[
                            ("↑/↓", "browse"),
                            ("←/→", "filter"),
                            ("esc", quit),
                            ("ctrl+c", "quit"),
                        ],
                        self.palette.secondary,
                        inset(footer).width,
                    ),
                    hints(
                        &[
                            ("ctrl+o", density),
                            ("ctrl+e", "details"),
                            ("pgup/pgdn", "page"),
                        ],
                        self.palette.secondary,
                        inset(footer).width,
                    ),
                ]
            };
            frame.render_widget(
                Paragraph::new(rows),
                inset(Rect::new(
                    footer.x,
                    footer.y.saturating_add(1),
                    footer.width,
                    footer.height.saturating_sub(1),
                )),
            );
        }
    }

    fn item(
        &self,
        row: &Row,
        observed_at: u64,
        width: u16,
        cols: Columns,
        index: usize,
    ) -> ListItem<'static> {
        let selected = self.list.selected() == Some(index);
        let normal = if selected {
            Style::new()
        } else {
            self.palette.secondary
        };
        let state_style = if selected {
            Style::new().bold()
        } else {
            self.palette.state(row.state)
        };
        let state = if cols.state == 6 {
            short_state(row.state)
        } else {
            row.state.label()
        };
        let title_width = usize::from(width).saturating_sub(cols.state + cols.time + cols.finished);
        let mut summary = vec![
            Span::styled(format!("{state:width$}", width = cols.state), state_style),
            Span::styled(
                format!(
                    "{:width$}",
                    clip(&run_time(row.run_time_ms), cols.time.saturating_sub(1)),
                    width = cols.time
                ),
                normal,
            ),
        ];
        if cols.finished > 0 {
            let finished = row
                .last_finished_at
                .map_or_else(|| "—".into(), |at| age(observed_at, at));
            summary.push(Span::styled(
                format!("{finished:width$}", width = cols.finished),
                normal,
            ));
        }
        summary.push(Span::styled(
            clip(
                row.thread_name.as_deref().unwrap_or("Unnamed thread"),
                title_width,
            ),
            if selected {
                Style::new().bold()
            } else {
                Style::new()
            },
        ));
        let mut lines = vec![Line::from(summary)];
        if !self.dense || (selected && self.expanded) {
            lines.push(
                Line::from(clip(
                    &format!(
                        "{}  ·  {}",
                        row.tty.as_deref().unwrap_or("no tty"),
                        row.cwd.as_deref().unwrap_or("—")
                    ),
                    width.into(),
                ))
                .style(normal),
            );
            let finished = row
                .last_finished_at
                .map_or_else(|| "—".into(), |at| age(observed_at, at));
            lines.push(
                Line::from(clip(
                    &format!("Last finished {finished}  ·  PID {}", row.pid),
                    width.into(),
                ))
                .style(normal),
            );
        }
        if selected && self.expanded {
            lines.push(
                Line::from(clip(
                    &format!("Thread {}", row.thread_id.as_deref().unwrap_or("—")),
                    width.into(),
                ))
                .style(normal),
            );
        }
        ListItem::new(lines)
    }
}

fn short_state(state: TurnState) -> &'static str {
    match state {
        TurnState::Working => "Run",
        TurnState::NeedsInput => "Input",
        TurnState::Ready => "Done",
        TurnState::Interrupted => "Stop",
        TurnState::Error => "Err",
        TurnState::Unknown => "?",
        TurnState::Exited => "Exit",
    }
}

#[derive(Clone, Copy)]
struct Columns {
    state: usize,
    time: usize,
    finished: usize,
}
impl Columns {
    fn new(width: u16) -> Self {
        Self {
            state: if width < 62 { 6 } else { 12 },
            time: if width < 62 { 9 } else { 11 },
            finished: if width >= 88 { 14 } else { 0 },
        }
    }
}

fn hints(values: &[(&str, &str)], secondary: Style, width: u16) -> Line<'static> {
    let mut spans = Vec::new();
    let mut used = 0;
    for (index, (key, label)) in values.iter().enumerate() {
        let added = key.width() + label.width() + 1 + if index > 0 { 3 } else { 0 };
        if used + added > usize::from(width) {
            break;
        }
        used += added;
        if index > 0 {
            spans.push(Span::raw("   "));
        }
        spans.push(Span::styled((*key).to_owned(), Style::new().bold()));
        spans.push(Span::styled(format!(" {label}"), secondary));
    }
    Line::from(spans)
}

fn clip(text: &str, width: usize) -> String {
    let clean = safe_text(text, usize::MAX);
    if clean.width() <= width {
        clean
    } else if width == 0 {
        String::new()
    } else {
        format!("{}…", safe_text(&clean, width - 1))
    }
}

pub(super) fn run_time(ms: Option<u64>) -> String {
    let Some(seconds) = ms.map(|value| value / 1000) else {
        return "—".into();
    };
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    } else {
        format!("{}h {:02}m", seconds / 3600, seconds % 3600 / 60)
    }
}

#[cfg(test)]
#[path = "ui_tests.rs"]
mod tests;
