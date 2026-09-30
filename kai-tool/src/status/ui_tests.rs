use super::*;
use ratatui::backend::TestBackend;

fn snapshot() -> Snapshot {
    Snapshot {
        version: 1,
        observed_at: 1000,
        warnings: vec![],
        windows: (0..20)
            .map(|i| Row {
                identity: ProcessIdentity {
                    pid: i + 10,
                    start_ticks: 42,
                },
                pid: i + 10,
                tty: Some(format!("pts/{i}")),
                cwd: Some("/workspace/kai".into()),
                thread_id: Some(format!("thread-{i}")),
                thread_name: Some(if i == 0 {
                    "Prepare release".into()
                } else {
                    format!("Task {i}")
                }),
                run_time_ms: Some(65000),
                last_finished_at: Some(900),
                exited_at: None,
                detail: None,
                agents: Some(super::super::agents::Summary {
                    complete: true,
                    ..super::super::agents::Summary::default()
                }),
                token_usage: None,
                state: if i == 0 {
                    TurnState::Working
                } else {
                    TurnState::Ready
                },
            })
            .collect(),
    }
}

fn view() -> View {
    View::new(Palette {
        selection: Style::new()
            .fg(Color::Rgb(0, 0, 46))
            .bg(Color::Rgb(99, 168, 248))
            .bold(),
        secondary: Style::new().dim(),
        colors: true,
    })
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn render(view: &mut View, snapshot: &Snapshot, width: u16, height: u16) -> Terminal<TestBackend> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| view.draw(frame, snapshot, Duration::from_secs(2)))
        .unwrap();
    terminal
}

fn contents(terminal: &Terminal<TestBackend>) -> String {
    let buffer = terminal.backend().buffer();
    (0..buffer.area.height)
        .map(|y| {
            let line: String = (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect();
            line.trim_end().to_owned() + "\n"
        })
        .collect()
}

#[test]
fn search_navigation_and_refresh_keep_the_selected_window() {
    let mut snapshot = snapshot();
    let mut view = view();
    view.reconcile(&snapshot);
    view.key(key(KeyCode::Down), &snapshot);
    let selected = view.selected;
    snapshot.windows.swap(0, 1);
    view.reconcile(&snapshot);
    assert_eq!(view.selected, selected);
    assert_eq!(view.list.selected(), Some(0));
    assert!(!view.key(key(KeyCode::Char('q')), &snapshot));
    assert_eq!(view.query, "q");
    assert!(view.visible.is_empty());
    assert!(!view.key(key(KeyCode::Esc), &snapshot));
    assert!(view.query.is_empty());
    view.add_query("PREPARE");
    view.reconcile(&snapshot);
    assert_eq!(view.visible.len(), 1);
    assert_eq!(view.selected.unwrap().pid, 10);
    view.key(key(KeyCode::Esc), &snapshot);
    view.key(key(KeyCode::Right), &snapshot);
    assert_eq!(view.visible.len(), 1);
    view.key(key(KeyCode::Right), &snapshot);
    assert_eq!(view.visible.len(), 20);
    view.key(key(KeyCode::Left), &snapshot);
    assert_eq!(view.visible.len(), 1);
    view.key(key(KeyCode::Left), &snapshot);
    assert_eq!(view.visible.len(), 20);
    assert!(view.key(
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        &snapshot
    ));
}

#[test]
fn layouts_show_name_runtime_and_navigation_without_disclaimer_text() {
    let mut snapshot = snapshot();
    snapshot.windows[0].token_usage = Some(super::super::tokens::Usage {
        input_tokens: 1_000_000,
        output_tokens: 24000,
        total_tokens: 1_024_000,
        cached_input_tokens: 500_000,
        cache_write_input_tokens: 0,
        reasoning_output_tokens: 1000,
    });
    for (width, height) in [(120, 24), (70, 12), (38, 12)] {
        let mut view = view();
        view.reconcile(&snapshot);
        let terminal = render(&mut view, &snapshot, width, height);
        let text = contents(&terminal);
        assert!(text.contains("Prepare release"), "{text}");
        assert!(text.contains("1m 05s"), "{text}");
        if width >= 70 {
            assert!(text.contains("Tokens") && text.contains("1.0M"), "{text}");
            assert!(text.contains("Agents"), "{text}");
        }
        assert!(text.contains("esc quit"), "{text}");
        assert!(text.contains("enter focus"), "{text}");
        assert!(text.contains("ctrl+o comfortable"), "{text}");
        assert!(!text.contains("recorded") && !text.contains("lag"));
        let buffer = terminal.backend().buffer();
        let title_cell = buffer
            .content
            .iter()
            .find(|cell| cell.symbol() == "P")
            .unwrap();
        assert_eq!(title_cell.bg, Color::Rgb(99, 168, 248));
        view.key(key(KeyCode::End), &snapshot);
        let terminal = render(&mut view, &snapshot, width, height);
        assert!(contents(&terminal).contains("Task 19"));
        assert!(contents(&terminal).contains("20 / 20"));
    }
}

#[test]
fn focus_uses_the_filtered_selection_and_reports_errors_only_on_request() {
    let mut snapshot = snapshot();
    let mut view = view();
    view.add_query("Task 19");
    view.reconcile(&snapshot);
    let expected = view.selected.unwrap();
    let message = "Window switching is not supported here. Press Ctrl+E for terminal details.";
    view.focus_selected(&snapshot, |identity| {
        assert_eq!(identity, expected);
        anyhow::bail!(message)
    });
    let text = contents(&render(&mut view, &snapshot, 38, 16));
    assert!(text.contains("Window switching"));
    assert!(text.contains("Ctrl+E"));
    assert!(text.contains("esc dismiss"));
    assert!(!text.contains("beta"));
    assert!(!view.key(key(KeyCode::Esc), &snapshot));
    assert!(view.notice.is_none());
    assert_eq!(view.query, "Task 19");
    view.focus_selected(&snapshot, |_| Ok(()));
    assert!(view.notice.is_none());
    assert_eq!(view.selected, Some(expected));
    snapshot.windows.pop();
    view.reconcile(&snapshot);
    assert!(view.selected.is_none());
    view.focus_selected(&snapshot, |_| panic!("cannot focus a removed window"));
    assert!(view.notice.is_none());
    view.add_query("no match");
    view.reconcile(&snapshot);
    view.focus_selected(&snapshot, |_| panic!("no selection"));
    assert!(view.notice.is_none());
}

#[test]
fn comfortable_view_and_expansion_show_terminal_directory_and_thread_id() {
    let snapshot = snapshot();
    let mut view = view();
    view.reconcile(&snapshot);
    view.key(
        KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL),
        &snapshot,
    );
    view.key(
        KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL),
        &snapshot,
    );
    let terminal = render(&mut view, &snapshot, 90, 24);
    let text = contents(&terminal);
    assert!(text.contains("pts/0  ·  /workspace/kai"));
    assert!(text.contains("Last ended 1m ago  ·  PID 10"));
    assert!(text.contains("Thread thread-0"));
    assert!(text.contains("ctrl+o dense"));
}

#[test]
fn tiny_windows_unicode_and_pastes_do_not_break_rendering() {
    let mut snapshot = snapshot();
    snapshot.windows[0].thread_name = Some("界界\u{1b}\n\u{202e}title".into());
    let mut view = view();
    view.reconcile(&snapshot);
    for (width, height) in [(1, 1), (10, 4), (40, 8)] {
        render(&mut view, &snapshot, width, height);
    }
    assert_eq!(clip("界界界", 5), "界界…");
    view.add_query(&"x".repeat(2000));
    assert_eq!(view.query.len(), 1024);
}

#[test]
fn expanded_details_expose_actual_errors_even_without_rows_and_can_scroll() {
    let mut snapshot = snapshot();
    let mut view = view();
    snapshot.windows.truncate(1);
    snapshot.windows[0].state = TurnState::Unknown;
    snapshot.windows[0].detail = Some("Cannot read the selected session log".into());
    view.reconcile(&snapshot);
    assert!(!contents(&render(&mut view, &snapshot, 70, 24)).contains("Cannot read"));
    view.key(
        KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL),
        &snapshot,
    );
    assert!(
        contents(&render(&mut view, &snapshot, 70, 24))
            .contains("Cannot read the selected session log")
    );
    snapshot.windows.clear();
    snapshot.warnings = vec!["Some process metadata could not be read".into()];
    view.refresh_error = Some("Refresh failed: permission denied".into());
    view.reconcile(&snapshot);
    let text = contents(&render(&mut view, &snapshot, 80, 24));
    assert!(text.contains("Stale · Ctrl+E details"));
    assert!(text.contains("permission denied"));
    assert!(text.contains("Some process metadata could not be read"));
    snapshot.warnings = (0..20).map(|i| format!("Warning {i}")).collect();
    render(&mut view, &snapshot, 40, 16);
    view.key(key(KeyCode::PageDown), &snapshot);
    assert!(view.detail_scroll > 0);
    let text = contents(&render(&mut view, &snapshot, 40, 16));
    assert!(text.contains("Warning"));
    assert!(!text.contains("Refresh failed: permission denied"));
    assert!(
        wrap_notice(&"界".repeat(20), 5)
            .iter()
            .all(|line| line.width() <= 5)
    );
}

#[test]
fn agents_column_and_active_filter_include_work_after_the_parent_is_ready() {
    let mut snapshot = snapshot();
    snapshot.windows.truncate(2);
    snapshot.windows[0].state = TurnState::Ready;
    snapshot.windows[0].agents = Some(super::super::agents::Summary {
        total: 4,
        running: 2,
        ready: 1,
        unknown: 1,
        complete: true,
        ..super::super::agents::Summary::default()
    });
    let mut view = view();
    view.filter = Filter::Active;
    view.reconcile(&snapshot);
    assert_eq!(view.visible, vec![0]);
    for width in [50, 70, 120] {
        let text = contents(&render(&mut view, &snapshot, width, 24));
        assert!(
            text.contains("Agents") && text.contains("2 active ?"),
            "{text}"
        );
        assert!(text.contains("Ready"), "{text}");
    }
    view.expanded = true;
    let text = contents(&render(&mut view, &snapshot, 38, 30));
    assert!(text.contains("Agents: 2 running"), "{text}");
    assert!(
        text.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .contains("1 unknown"),
        "{text}"
    );
    snapshot.windows[0].agents.as_mut().unwrap().running = 0;
    view.reconcile(&snapshot);
    assert!(view.visible.is_empty());
}

#[test]
fn token_details_show_exact_usage_and_do_not_confuse_missing_counts_with_zero() {
    let mut snapshot = snapshot();
    snapshot.windows.truncate(1);
    let mut view = view();
    view.expanded = true;
    view.reconcile(&snapshot);
    assert!(contents(&render(&mut view, &snapshot, 90, 40)).contains("Tokens: unavailable"));
    snapshot.windows[0].token_usage = Some(super::super::tokens::Usage {
        input_tokens: 1000,
        output_tokens: 240,
        total_tokens: 1240,
        cached_input_tokens: 500,
        cache_write_input_tokens: 100,
        reasoning_output_tokens: 80,
    });
    let text = contents(&render(&mut view, &snapshot, 90, 40));
    assert!(text.contains("1.2K"), "{text}");
    assert!(
        text.contains("1240 total · 1000 input · 240 output"),
        "{text}"
    );
    assert!(text.contains("500 cached · 100 cache write"), "{text}");
    assert!(text.contains("80 (included in output)"), "{text}");
    snapshot.windows[0]
        .token_usage
        .as_mut()
        .unwrap()
        .total_tokens = 0;
    assert_eq!(snapshot.windows[0].token_usage.unwrap().label(), "0");
}
