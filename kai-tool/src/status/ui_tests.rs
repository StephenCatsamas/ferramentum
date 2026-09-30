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
    assert_eq!(view.visible.len(), 19);
    assert!(view.key(
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        &snapshot
    ));
}

#[test]
fn layouts_show_name_runtime_and_navigation_without_disclaimer_text() {
    let snapshot = snapshot();
    for (width, height) in [(120, 24), (70, 12), (38, 12)] {
        let mut view = view();
        view.reconcile(&snapshot);
        let terminal = render(&mut view, &snapshot, width, height);
        let text = contents(&terminal);
        assert!(text.contains("Prepare release"), "{text}");
        assert!(text.contains("1m 05s"), "{text}");
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
    snapshot.windows.last_mut().unwrap().exited_at = Some(999);
    view.focus_selected(&snapshot, |_| panic!("cannot focus exited window"));
    assert!(view.notice.as_deref().unwrap().contains("exited"));
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
    assert!(text.contains("Last finished 1m ago  ·  PID 10"));
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
